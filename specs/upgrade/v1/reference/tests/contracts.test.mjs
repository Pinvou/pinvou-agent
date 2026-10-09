import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { generateKeyPairSync } from 'node:crypto';
import { canonicalize, parseJson } from '../canonical-json.mjs';
import { schemas } from '../build-schemas.mjs';
import { sha256, rolloutCommitment, assertByteIdentity, assertNativeIdentity } from '../digests.mjs';
import { validateSchema } from '../schema-registry.mjs';
import { verifyEnvelope, signEnvelope, publicKeyDescriptor, verifyRootSuccessor,
  verifyRootChain, assertReleaseRenewal, verifyArchivedPackage } from '../signatures.mjs';
import { checkSemantics, assertBindings, assertOccurrence, assertOpening, DAY,
  METADATA_LIFETIMES, CREDENTIAL_LIFETIMES, consumeQueryAvailable, outcomeRetentionRequired,
  selectFullPackage } from '../semantics.mjs';
import { verifyPublicChain, verifyCandidateRelease, verifyReleasePackage } from '../metadata-chain.mjs';
import { createFixtureSet, NOW, TARGET, HASH } from './fixtures.mjs';

const fixtures = createFixtureSet();
const expected = (claims) => ({ role: claims.role, product: claims.product, component: claims.component, scope: claims.scope });
const verify = (bytes, claims, options = {}) => verifyEnvelope(bytes,
  { trustedRoot: fixtures.metadata.root, expected: expected(claims), now: NOW, ...options });
const rejects = (operation, code) => assert.throws(operation, code ? { code } : undefined);

test('committed schemas match their reproducible source and compile without remote references', async () => {
  for (const [name, schema] of schemas) {
    const artifact = await readFile(new URL(`../../schemas/${name}.json`, import.meta.url), 'utf8');
    assert.equal(artifact, JSON.stringify(schema, null, 2) + '\n');
  }
});

test('six metadata roles and fourteen distinct credential roles have usable exact schemas', () => {
  for (const claims of [...Object.values(fixtures.metadata), ...Object.values(fixtures.claims)]) {
    const envelope = verify(fixtures.sign(claims), claims);
    assert.deepEqual(JSON.parse(JSON.stringify(envelope.signed)), JSON.parse(JSON.stringify(claims)));
    const unknown = structuredClone(envelope); unknown.signed.unknownCriticalField = true;
    rejects(() => verify(canonicalize(unknown), claims), 'SCHEMA_INVALID');
  }
});

test('every required credential field is required; each authenticated top-level binding cannot be changed', () => {
  for (const claims of Object.values(fixtures.claims)) {
    const original = parseJson(fixtures.sign(claims));
    for (const field of Object.keys(claims)) {
      const missing = structuredClone(original); delete missing.signed[field];
      rejects(() => verify(canonicalize(missing), claims), 'SCHEMA_INVALID');
      const changed = structuredClone(original);
      const value = changed.signed[field];
      changed.signed[field] = typeof value === 'number' ? value + 1
        : typeof value === 'string' ? value + '-changed' : value === null ? {} : null;
      rejects(() => verify(canonicalize(changed), claims));
    }
  }
});

test('role/product/component/scope and caller field binding prevent cross-context use', () => {
  const claims = fixtures.claims.decision;
  const bytes = fixtures.sign(claims);
  for (const change of [{ product: 'other' }, { component: 'other' }, { scope: { ...claims.scope, installationScopeId: 'other' } }]) {
    rejects(() => verify(bytes, claims, { expected: { ...expected(claims), ...change } }), 'CONTEXT_MISMATCH');
  }
  rejects(() => verify(bytes, claims, { expected: { ...expected(claims), role: 'download' } }), 'SCHEMA_INVALID');
  rejects(() => assertBindings(claims.scope, { ...claims.scope, channelRevision: 2 }), 'BINDING_MISMATCH');
  rejects(() => verifyEnvelope(bytes, { trustedRoot: fixtures.metadata.root, now: NOW }), 'EXPECTED_CONTEXT_REQUIRED');
});

test('distinct keys count once; untrusted keys and malformed signatures fail closed', () => {
  const root = fixtures.metadata.root;
  rejects(() => verify(signEnvelope(root, [fixtures.keys[0]]), root), 'SIGNATURE_THRESHOLD_UNMET');
  const duplicate = parseJson(fixtures.bytes.root);
  duplicate.signatures = [duplicate.signatures[0], duplicate.signatures[0]];
  rejects(() => verify(canonicalize(duplicate), root), 'SIGNATURE_ORDER_INVALID');
  const attacker = generateKeyPairSync('ed25519').privateKey;
  rejects(() => verify(signEnvelope(fixtures.metadata.release, [attacker]), fixtures.metadata.release), 'SIGNING_KEY_NOT_AUTHORIZED');
  const malformed = parseJson(fixtures.bytes.release); malformed.signatures[0].signature = 'A'.repeat(86);
  rejects(() => verify(canonicalize(malformed), fixtures.metadata.release), 'SIGNATURE_INVALID');
  rejects(() => verify(fixtures.bytes.release, fixtures.metadata.release,
    { deniedKeyIds: [fixtures.descriptors[3].keyId] }), 'SIGNING_KEY_DENIED');
  const badRoot = structuredClone(root); badRoot.keys[0].algorithm = 'unknown';
  rejects(() => verify(fixtures.bytes.release, fixtures.metadata.release, { trustedRoot: badRoot }), 'SCHEMA_INVALID');
});

test('envelope identity and high-water use the exact canonical complete envelope', () => {
  const claims = fixtures.metadata.release;
  const bytes = fixtures.bytes.release;
  const highWater = { scope: expected(claims), version: 1, envelopeSha256: sha256(bytes) };
  verify(bytes, claims, { highWater });
  rejects(() => verify(bytes, claims, { highWater: { ...highWater, version: 2 } }), 'METADATA_ROLLBACK');
  rejects(() => verify(bytes, claims, { highWater: { ...highWater, envelopeSha256: HASH } }), 'METADATA_EQUIVOCATION');
  rejects(() => verify(Buffer.concat([Buffer.from(bytes), Buffer.from('\n')]), claims), 'ENVELOPE_NOT_CANONICAL');
});

test('Package has no expiry/self hash/upload hash; historical identity remains verifiable', () => {
  const packageManifest = fixtures.metadata.package;
  for (const field of ['expiresAt', 'manifestEnvelopeSha256', 'uploadZipSha256', 'zipSha256']) {
    const claims = { ...packageManifest, [field]: field === 'expiresAt' ? NOW : HASH };
    rejects(() => validateSchema('metadata/package', { signed: claims, signatures: parseJson(fixtures.bytes.package).signatures }), 'SCHEMA_INVALID');
  }
  const farFuture = NOW + 500 * DAY;
  rejects(() => verify(fixtures.bytes.package, packageManifest, { now: farFuture }), 'TIME_WINDOW_CLOSED');
  verifyArchivedPackage(fixtures.bytes.package, { archivedRoot: fixtures.metadata.root, expected: expected(packageManifest), now: farFuture });
  rejects(() => verifyArchivedPackage(fixtures.bytes.release,
    { archivedRoot: fixtures.metadata.root, expected: expected(fixtures.metadata.release), now: farFuture }), 'ARCHIVE_ROLE_INVALID');
});

test('incremental fixtures are parsed only in isolation; empty capability always selects the full package', () => {
  const claims = structuredClone(fixtures.metadata.package);
  claims.incrementalPackages = [{ packageId: 'delta-test', size: 12, sha256: HASH,
    baseVersion: '1.0.0', baseArtifactSha256: HASH, resultArtifactSize: 123, resultArtifactSha256: HASH,
    algorithm: 'test-only', algorithmVersion: '1', format: 'test-only' }];
  const bytes = fixtures.sign(claims);
  rejects(() => verify(bytes, claims), 'INCREMENTAL_PRODUCTION_DISABLED');
  verify(bytes, claims, { production: false });
  assert.deepEqual(selectFullPackage(claims, []), claims.fullPackage);
});

test('all bounded windows reject exact expiry and any excess declaration', () => {
  for (const [role, maximum] of Object.entries(METADATA_LIFETIMES)) {
    const claims = structuredClone(fixtures.metadata[role]);
    claims.expiresAt = claims.issuedAt + maximum;
    checkSemantics(claims, { now: claims.expiresAt - 1 });
    for (const now of [claims.expiresAt, claims.expiresAt + 1]) rejects(() => checkSemantics(claims, { now }), 'TIME_WINDOW_CLOSED');
    claims.expiresAt++; rejects(() => checkSemantics(claims, { now: NOW }), 'TIME_WINDOW_INVALID');
  }
  for (const [role, maximum] of Object.entries(CREDENTIAL_LIFETIMES)) {
    const claims = structuredClone(fixtures.claims[role]);
    // Qualification metadata expires before long uploads, but event roles do not
    // carry install qualification. Short credentials remain inside that window.
    claims.exp = claims.iat + maximum;
    checkSemantics(claims, { now: claims.exp - 1 });
    for (const now of [claims.exp, claims.exp + 1]) rejects(() => checkSemantics(claims, { now }), 'TIME_WINDOW_CLOSED');
    claims.exp++; rejects(() => checkSemantics(claims, { now: NOW }), 'TIME_WINDOW_INVALID');
  }
});

test('events cannot move original anchors or occurrence/upload windows', () => {
  for (const claims of Object.values(fixtures.claims).filter((claim) => claim.role.endsWith('-event'))) {
    assertOccurrence(claims, claims.eventNotAfter - 1, claims.eventNotAfter);
    for (const occurredAt of [claims.nbf - 1, claims.eventNotAfter, claims.eventNotAfter + 1]) {
      rejects(() => assertOccurrence(claims, occurredAt, claims.eventNotAfter + 1), 'EVENT_OCCURRENCE_INVALID');
    }
    const moved = structuredClone(claims); moved.eventNotAfter++;
    rejects(() => checkSemantics(moved, { now: NOW }), 'EVENT_WINDOW_INVALID');
  }
  for (const offset of [61, 62]) {
    const check = offset === 61 ? consumeQueryAvailable : outcomeRetentionRequired;
    assert.equal(check(NOW, NOW + offset * DAY - 1), true);
    assert.equal(check(NOW, NOW + offset * DAY), false);
    assert.equal(check(NOW, NOW + offset * DAY + 1), false);
  }
});

test('three authorization purposes enforce preparation/freeze/backup/staged bindings', () => {
  for (const purpose of ['preinstall', 'install', 'activate']) {
    const claims = structuredClone(fixtures.claims[`authorization-${purpose}`]);
    checkSemantics(claims, { now: NOW });
    claims.purpose = purpose === 'install' ? 'activate' : 'install';
    rejects(() => fixtures.sign(claims), 'SCHEMA_INVALID');
  }
  const install = structuredClone(fixtures.claims['authorization-install']);
  install.update.migrationMode = 'irreversible';
  install.update.plans.migration = { planId: 'migration-2', revision: 1, sha256: HASH };
  rejects(() => checkSemantics(install, { now: NOW }), 'BACKUP_POLICY_INVALID');
  install.update.backupPolicy = 'required';
  install.update.plans.backup = { planId: 'backup-2', revision: 1, sha256: HASH };
  rejects(() => checkSemantics(install, { now: NOW }), 'BACKUP_BINDING_INVALID');
  install.backup = { recordId: 'backup-1', freezeEpoch: 1, sha256: HASH, dataScopeDigest: HASH, preparationEpoch: 1 };
  checkSemantics(install, { now: NOW });
  install.backup.preparationEpoch++;
  rejects(() => checkSemantics(install, { now: NOW }), 'BACKUP_BINDING_INVALID');
  const preinstall = structuredClone(fixtures.claims['authorization-preinstall']); preinstall.freezeEpoch = 1;
  rejects(() => checkSemantics(preinstall, { now: NOW }), 'FREEZE_BINDING_INVALID');
  const notYetInstallable = structuredClone(fixtures.claims['authorization-install']);
  notYetInstallable.update.endpointChain.installNotBefore++; notYetInstallable.update.hopChain.installNotBefore++;
  rejects(() => checkSemantics(notYetInstallable, { now: NOW }), 'INSTALL_NOT_YET_VALID');
});

test('baseline/candidate by ordinary/bridge combinations have exclusive bindings', () => {
  for (const endpointKind of ['baseline', 'candidate']) {
    for (const hopKind of ['ordinary', 'bridge']) {
      const claims = structuredClone(fixtures.claims.decision);
      Object.assign(claims.update, { endpointKind, hopKind });
      if (endpointKind === 'candidate') claims.update.rollout = { rolloutId: 'rollout-1', revision: 1,
        state: 'running', policyRevision: 1, opening: { targetKey: TARGET, deploymentId: 'deployment-2',
          deploymentRevision: 1, rolloutId: 'rollout-1', rolloutRevision: 1,
          releaseEnvelopeSha256: sha256(fixtures.bytes.release), leafSalt: HASH }, locator: 'opaque-1', locatorExpiresAt: claims.exp };
      if (hopKind === 'bridge') {
        claims.update.bridge = { eligibilityId: 'bridge-1', revision: 1 };
        claims.update.hopChain.deploymentId = 'historical-hop';
        claims.update.hopChain.deploymentState = 'superseded';
      }
      checkSemantics(claims, { now: NOW });
      const changed = structuredClone(claims);
      if (endpointKind === 'candidate') changed.update.rollout = null;
      else changed.update.rollout = {};
      rejects(() => checkSemantics(changed, { now: NOW }), 'ROLLOUT_BINDING_INVALID');
      const bridgeChanged = structuredClone(claims); bridgeChanged.update.bridge = hopKind === 'bridge' ? null : {};
      rejects(() => checkSemantics(bridgeChanged, { now: NOW }), 'BRIDGE_BINDING_INVALID');
    }
  }
});

test('public snapshot membership cannot expose an unrelated signed candidate', () => {
  const args = { trustedRoot: fixtures.metadata.root, product: 'pinvou', component: 'app', now: NOW,
    timestampBytes: fixtures.bytes.timestamp, snapshotBytes: fixtures.bytes.snapshot,
    members: [fixtures.bytes.target, fixtures.bytes.release, fixtures.bytes.package] };
  verifyPublicChain(args);
  const snapshot = structuredClone(fixtures.metadata.snapshot);
  snapshot.entries[0].size++;
  const snapshotBytes = fixtures.sign(snapshot);
  const timestamp = structuredClone(fixtures.metadata.timestamp);
  timestamp.snapshot.envelopeSha256 = sha256(snapshotBytes); timestamp.snapshot.size = snapshotBytes.length;
  rejects(() => verifyPublicChain({ ...args, snapshotBytes, timestampBytes: fixtures.sign(timestamp) }), 'REFERENCE_MISMATCH');
  const candidate = structuredClone(fixtures.metadata.release); candidate.scope.releaseId = 'private-candidate';
  const candidateBytes = fixtures.sign(candidate);
  rejects(() => verifyPublicChain({ ...args, members: [...args.members, candidateBytes] }), 'PUBLIC_MEMBER_UNREFERENCED');
});

test('candidate requires a current correct salted opening even when its signature is valid', () => {
  const opening = { targetKey: TARGET, deploymentId: 'deployment-2', deploymentRevision: 1,
    rolloutId: 'rollout-1', rolloutRevision: 1, releaseEnvelopeSha256: sha256(fixtures.bytes.release), leafSalt: HASH };
  const target = structuredClone(fixtures.metadata.target); target.rolloutSetCommitment = rolloutCommitment(opening);
  // The selected candidate is a different signed Release, not the baseline.
  const candidatePackage = structuredClone(fixtures.metadata.package);
  candidatePackage.appVersion = '3.0.0'; candidatePackage.fullPackage.packageId = 'package-3';
  const candidatePackageBytes = fixtures.sign(candidatePackage);
  const candidate = structuredClone(fixtures.metadata.release); candidate.scope.releaseId = 'candidate-3'; candidate.appVersion = '3.0.0';
  candidate.targets[0].packageManifest = { ...candidate.targets[0].packageManifest,
    size: candidatePackageBytes.length, envelopeSha256: sha256(candidatePackageBytes) };
  const candidateBytes = fixtures.sign(candidate);
  opening.releaseEnvelopeSha256 = sha256(candidateBytes);
  target.rolloutSetCommitment = rolloutCommitment(opening);
  const args = { trustedRoot: fixtures.metadata.root, product: 'pinvou', component: 'app', targetBytes: fixtures.sign(target),
    channel: 'stable', targetKey: TARGET, releaseBytes: candidateBytes, expectedReleaseId: 'candidate-3', opening, now: NOW };
  const accepted = verifyCandidateRelease(args);
  verifyReleasePackage({ trustedRoot: fixtures.metadata.root, product: 'pinvou', component: 'app',
    release: accepted, packageBytes: candidatePackageBytes, targetKey: TARGET, now: NOW });
  for (const field of Object.keys(opening)) {
    const changed = { ...opening, [field]: typeof opening[field] === 'number' ? opening[field] + 1
      : field.endsWith('Sha256') || field === 'leafSalt' ? 'b'.repeat(64) : opening[field] + '-other' };
    rejects(() => assertOpening(target, sha256(candidateBytes), changed));
  }
  rejects(() => assertOpening(target, 'b'.repeat(64), opening), 'OPENING_MISMATCH');
});

test('byte and native identities independently bind size/hash and publisher', () => {
  const bytes = Buffer.from('test-artifact');
  const identity = { size: bytes.length, sha256: sha256(bytes) };
  assertByteIdentity(identity, bytes);
  rejects(() => assertByteIdentity(identity, Buffer.from('other-artifact')), 'ARTIFACT_BYTES_MISMATCH');
  const native = fixtures.metadata.package.finalInstaller;
  assertNativeIdentity(native, { ...native });
  rejects(() => assertNativeIdentity(native, { ...native, publisher: 'other-publisher' }), 'NATIVE_IDENTITY_MISMATCH');
});

test('Root publication pure guard preserves verification for every supplied active component', () => {
  const auxiliary = createFixtureSet({ component: 'engine' });
  const current = structuredClone(fixtures.metadata.root);
  current.keys.push(auxiliary.descriptors[3]);
  current.roles.push(...auxiliary.metadata.root.roles.filter((role) => role.role !== 'root'));
  const next = structuredClone(current); next.version = 2;
  const activeEnvelopes = [...Object.values(fixtures.bytes), ...Object.values(auxiliary.bytes)]
    .filter((bytes) => parseJson(bytes).signed.role !== 'root')
    .map((bytes) => ({ bytes, expected: expected(parseJson(bytes).signed) }));
  const options = { trustedRoot: current, product: 'pinvou', now: NOW, activeEnvelopes };
  verifyRootSuccessor(fixtures.sign(next), options);
  const broken = structuredClone(next);
  broken.roles = broken.roles.filter((role) => role.component !== 'engine');
  broken.keys = broken.keys.filter((key) => key.keyId !== auxiliary.descriptors[3].keyId);
  rejects(() => verifyRootSuccessor(fixtures.sign(broken), options), 'ROLE_NOT_AUTHORIZED');
});

test('Root successors need old and new thresholds, continuity and final validity', () => {
  const newKeys = Array.from({ length: 3 }, () => generateKeyPairSync('ed25519').privateKey);
  const next = structuredClone(fixtures.metadata.root); next.version = 2;
  next.keys = [...next.keys, ...newKeys.map(publicKeyDescriptor)];
  next.roles.find((role) => role.role === 'root').keyIds = newKeys.map((key) => publicKeyDescriptor(key).keyId);
  const options = { trustedRoot: fixtures.metadata.root, product: 'pinvou', now: NOW };
  verifyRootSuccessor(signEnvelope(next, [...fixtures.keys.slice(0, 2), ...newKeys.slice(0, 2)]), options);
  rejects(() => verifyRootSuccessor(signEnvelope(next, newKeys.slice(0, 2)), options), 'SIGNATURE_THRESHOLD_UNMET');
  const skipped = { ...next, version: 3 };
  rejects(() => verifyRootSuccessor(signEnvelope(skipped, [...fixtures.keys.slice(0, 2), ...newKeys.slice(0, 2)]), options), 'ROOT_CONTINUITY_INVALID');
  const renewed = structuredClone(fixtures.metadata.release); renewed.revision = 2; renewed.issuedAt++; renewed.expiresAt++;
  assertReleaseRenewal(fixtures.metadata.release, renewed);
  renewed.notes.en = 'Changed business content';
  rejects(() => assertReleaseRenewal(fixtures.metadata.release, renewed), 'RELEASE_BUSINESS_CHANGED');
});

test('an expired cached Root can traverse expired intermediates but never produce new rights before a valid final Root', () => {
  const root2 = structuredClone(fixtures.metadata.root); root2.version = 2;
  root2.issuedAt += 100 * DAY; root2.expiresAt = root2.issuedAt + 365 * DAY;
  const root3 = structuredClone(root2); root3.version = 3;
  root3.issuedAt += 365 * DAY; root3.expiresAt = root3.issuedAt + 365 * DAY;
  const now = NOW + 500 * DAY;
  const options = { trustedRoot: fixtures.metadata.root, product: 'pinvou', now };
  verifyRootChain([fixtures.sign(root2), fixtures.sign(root3)], options);
  rejects(() => verifyRootChain([fixtures.sign(root2)], options), 'TIME_WINDOW_CLOSED');
  rejects(() => verifyRootChain([fixtures.sign(root3)], options), 'ROOT_CONTINUITY_INVALID');
  rejects(() => verify(fixtures.sign(root2), root2), 'ROOT_TRANSITION_REQUIRED');
});
