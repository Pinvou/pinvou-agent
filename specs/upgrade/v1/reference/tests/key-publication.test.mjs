import test from 'node:test';
import assert from 'node:assert/strict';
import { applyAtomically, captureRecord } from '../models/atomic.mjs';
import { planPublishRoot, planEmergencyDeny } from '../models/publication.mjs';
import { envelopeReference, signEnvelope, verifyEnvelope } from '../signatures.mjs';
import { canonicalize, parseJson } from '../canonical-json.mjs';
import { verifyPublicChain } from '../metadata-chain.mjs';
import { objectHash } from '../models/inputs.mjs';
import { releaseBusinessHash } from '../models/metadata-inputs.mjs';
import { publicationFixture, approveKeyCommand } from './publication-fixtures.mjs';
import { operation, operationCommand } from './model-fixtures.mjs';
import { NOW } from './fixtures.mjs';
import { DAY } from '../semantics.mjs';

function rootFixture(role = 'telemetry-event') {
  const fixture = publicationFixture(); const records = fixture.records; const claims = fixture.set.claims[role];
  const policy = records.root.body.roles.find((entry) => entry.role === role);
  records.components = { revision: 1, componentHeadKeys: ['head'] };
  records.issuer = { revision: 1, recordKind: 'issuerHead', lastIssuedAt: NOW, objectSha256: objectHash(claims) };
  records.use = { revision: 1, recordKind: 'keyUse', projectionOwner: 'T07', product: 'pinvou',
    component: policy.component, role, channel: policy.channel, targetKey: policy.targetKey,
    keys: policy.keyIds.map((id) => records.root.body.keys.find((key) => key.keyId === id)), threshold: policy.threshold,
    issuanceState: 'stopped', lastIssuedAt: NOW, objectValidUntil: claims.exp, uploadUntil: claims.exp,
    recoveryUntil: NOW + DAY, retainUntil: claims.exp + 120_000,
    readSet: [{ ...captureRecord(records, 'issuer'), recordKind: 'issuerHead' }] };
  records['key-uses'] = { revision: 1, recordKind: 'keyUseIndex', projectionOwner: 'T07', product: 'pinvou', usageKeys: ['use'] };
  const successor = { ...structuredClone(records.root.body), version: 2, issuedAt: NOW + 1 };
  const command = { ...operationCommand, rootKey: 'root', activeComponentsKey: 'components',
    baseRoot: captureRecord(records, 'root'), successorBytes: fixture.set.sign(successor) };
  approveKeyCommand(records, command, NOW + 1);
  return { ...fixture, claims, successor, command };
}

test('Root preserves old credential role/scope/threshold until the longest window plus two minutes', () => {
  for (const role of ['telemetry-event', 'install-transaction-event']) {
    const fixture = rootFixture(role); const { records, successor, command, claims } = fixture;
    const policy = successor.roles.find((entry) => entry.role === role);
    const alternate = successor.roles.find((entry) => entry.role === 'root').keyIds[0];
    policy.keyIds = [alternate]; command.successorBytes = fixture.set.sign(successor); approveKeyCommand(records, command, NOW + 1);
    assert.throws(() => planPublishRoot(records, command, NOW + 1), { code: 'MODEL_KEY_RETENTION_INVALID' });
    const oldKey = claims.signingKeyId; policy.keyIds = [oldKey, alternate]; policy.threshold = 2;
    command.successorBytes = fixture.set.sign(successor); approveKeyCommand(records, command, NOW + 1);
    assert.throws(() => planPublishRoot(records, command, NOW + 1), { code: 'MODEL_KEY_RETENTION_INVALID' });
    policy.threshold = 1; command.successorBytes = fixture.set.sign(successor); approveKeyCommand(records, command, NOW + 1);
    const plan = planPublishRoot(records, command, NOW + 1); const result = applyAtomically(records, plan, NOW + 1).records;
    verifyEnvelope(fixture.set.sign(claims), { trustedRoot: result.root.body, now: claims.exp - 1,
      expected: { role, product: claims.product, component: claims.component, scope: claims.scope } });
    for (const key of ['issuer', 'use', 'key-uses']) {
      const raced = structuredClone(records); raced[key].revision++;
      assert.throws(() => applyAtomically(raced, plan, NOW + 1), { code: 'MODEL_CAS_CONFLICT' });
    }
    policy.keyIds = [alternate]; const end = records.use.retainUntil;
    for (const now of [end - 1, end]) {
      records.operation = operation(now); successor.issuedAt = now; successor.expiresAt = now + 365 * DAY;
      command.successorBytes = fixture.set.sign(successor); approveKeyCommand(records, command, now);
      if (now < end) assert.throws(() => planPublishRoot(records, command, now), { code: 'MODEL_KEY_RETENTION_INVALID' });
      else assert.equal(applyAtomically(records, planPublishRoot(records, command, now), now).records.root.body.version, 2);
    }
  }
});

test('Root denies incomplete key-use inventories, active signing exit and invalid retention bounds', () => {
  const fixture = rootFixture(); const { records, command, successor } = fixture;
  for (const mutate of [(copy) => { copy['key-uses'].usageKeys = []; },
    (copy) => { copy.use.retainUntil = copy.use.uploadUntil; },
    (copy) => { copy.use.readSet[0].sha256 = 'b'.repeat(64); }]) {
    const copy = structuredClone(records); mutate(copy); assert.throws(() => planPublishRoot(copy, command, NOW + 1));
  }
  const now = records.use.retainUntil; records.use.issuanceState = 'active'; records.operation = operation(now);
  successor.issuedAt = now; successor.expiresAt = now + 365 * DAY;
  successor.roles.find((policy) => policy.role === 'telemetry-event').keyIds = [successor.roles.find((policy) => policy.role === 'root').keyIds[0]];
  command.successorBytes = fixture.set.sign(successor); approveKeyCommand(records, command, now);
  assert.throws(() => planPublishRoot(records, command, now), { code: 'MODEL_KEY_RETENTION_INVALID' });
});

test('Root successor lineage binds the unchanged initial anchor and exact immutable chain material', () => {
  const { records, command } = rootFixture(); const plan = planPublishRoot(records, command, NOW + 1);
  const lineage = records[command.successorLineageKey];
  for (const key of [command.successorLineageKey, lineage.anchorKey, lineage.chainMaterialKey, records.root.rootLineageKey]) {
    const raced = structuredClone(records); raced[key].revision++;
    assert.throws(() => applyAtomically(raced, plan, NOW + 1), { code: 'MODEL_CAS_CONFLICT' });
  }
  for (const field of ['rootBodySha256', 'anchorBodySha256', 'chainMaterialSha256']) {
    const copy = structuredClone(records); copy[command.successorLineageKey][field] = 'b'.repeat(64);
    assert.throws(() => planPublishRoot(copy, command, NOW + 1));
  }
});

test('Root and emergency key deny require exact scope/body/revision, MFA and two independent reviewers', () => {
  const fixture = rootFixture();
  const commands = [fixture.command, { ...operationCommand, denyKey: 'deny', inventoryKey: 'deny-inventory',
    affectedScopeKeys: ['scope'], jobKey: 'deny-job', denyEntries: [{ subjectKind: 'signingKey',
      subjectId: fixture.claims.signingKeyId, roles: ['telemetry-event'] }] }];
  for (const command of commands) {
    const records = structuredClone(fixture.records);
    records['deny-inventory'] = { revision: 1, product: 'pinvou', component: 'app', affectedScopeKeys: ['scope'] };
    approveKeyCommand(records, command, NOW + 1);
    const planner = command.successorBytes ? planPublishRoot : planEmergencyDeny;
    const plan = planner(records, command, NOW + 1);
    for (const mutate of [(copy) => { delete copy['key-permission']; }, (copy) => { copy['key-permission'].mfaVerified = false; },
      (copy) => { copy['key-permission'].state = 'revoked'; }, (copy) => { copy['key-permission'].expiresAt = NOW + 1; },
      (copy) => { copy['key-approval'].bodySha256 = 'b'.repeat(64); },
      (copy) => { copy['key-approval'].context.scope.component = 'other'; },
      (copy) => { copy['key-approval'].context.objectRevision++; },
      (copy) => { copy['key-approval'].reviewerIds = ['reviewer-a']; },
      (copy) => { copy['key-approval'].reviewerIds = ['reviewer-a', 'operator']; }]) {
      const copy = structuredClone(records); mutate(copy); assert.throws(() => planner(copy, command, NOW + 1));
    }
    for (const key of ['key-permission', 'key-approval']) {
      const raced = structuredClone(records); raced[key].revision++;
      assert.throws(() => applyAtomically(raced, plan, NOW + 1), { code: 'MODEL_CAS_CONFLICT' });
    }
    const changed = { ...command, actorId: 'other-operator' }; assert.throws(() => planner(records, changed, NOW + 1));
  }
});

test('Root-role deny rejects genesis/rotation signers and deny changes invalidate prepared publication', () => {
  for (const genesis of [false, true]) {
    const fixture = rootFixture(); const { records, command } = fixture;
    if (genesis) {
      Object.assign(records.root, { revision: 0, published: false }); Object.assign(records.head, { revision: 0, published: false });
      records.anchor = { revision: 1, recordKind: 'initialRootAnchor', state: 'provisioned', product: 'pinvou', bodySha256: objectHash(records.root.body) };
      command.anchorKey = 'anchor'; command.baseRoot = captureRecord(records, 'root'); command.successorBytes = fixture.set.bytes.root;
      approveKeyCommand(records, command, NOW + 1);
    }
    const plan = planPublishRoot(records, command, NOW + 1);
    const denied = structuredClone(records); denied.deny.revision++;
    denied.deny.entries.push({ subjectKind: 'signingKey', subjectId: fixture.set.metadata.root.roles.find((entry) => entry.role === 'root').keyIds[0], roles: ['root'] });
    assert.throws(() => applyAtomically(denied, plan, NOW + 1), { code: 'MODEL_CAS_CONFLICT' });
    assert.throws(() => planPublishRoot(denied, command, NOW + 1), { code: 'SIGNING_KEY_DENIED' });
    denied.deny.entries[0].roles = ['telemetry-event'];
    assert.equal(applyAtomically(denied, planPublishRoot(denied, command, NOW + 1), NOW + 1).records.root.published, true);
  }
});

function metadataRotation(fixture, role, now) {
  const { records, set } = fixture;
  records.components = { revision: 1, componentHeadKeys: ['head'] };
  records.operation = operation(now);
  const successor = { ...structuredClone(records.root.body), version: records.root.body.version + 1,
    issuedAt: now, expiresAt: now + 365 * DAY };
  if (role !== null) successor.roles.find((policy) => policy.role === role).keyIds = [set.descriptors[0].keyId];
  const command = { ...operationCommand, rootKey: 'root', activeComponentsKey: 'components',
    baseRoot: captureRecord(records, 'root'), successorBytes: set.sign(successor) };
  approveKeyCommand(records, command, now);
  return command;
}

test('Root compatibility ends separately at each signed metadata expiry boundary', () => {
  for (const role of ['timestamp', 'snapshot', 'target', 'release']) {
    for (const offset of [-1, 0, 1]) {
      const fixture = publicationFixture();
      const now = fixture.set.metadata[role].expiresAt + offset;
      const command = metadataRotation(fixture, role, now);
      if (offset < 0) {
        assert.throws(() => planPublishRoot(fixture.records, command, now), { code: 'SIGNING_KEY_NOT_AUTHORIZED' });
      } else {
        const before = structuredClone(fixture.records); const beforeHash = objectHash(fixture.records);
        const plan = planPublishRoot(fixture.records, command, now);
        assert.equal(objectHash(fixture.records), beforeHash);
        const result = applyAtomically(fixture.records, plan, now).records;
        assert.deepEqual(result.head, before.head);
        assert.deepEqual(result['verification-archive'], before['verification-archive']);
        assert.ok(plan.readSet.some((read) => read.key === 'head'
          && read.sha256 === captureRecord(before, 'head').sha256));
        assert.equal(result.root.body.version, 2);
      }
    }
  }
});

test('expired Timestamp cannot suppress unexpired Snapshot, Target or Release compatibility', () => {
  for (const role of ['snapshot', 'target', 'release']) {
    const fixture = publicationFixture();
    const timestamp = { ...fixture.set.metadata.timestamp, expiresAt: NOW + 1000 };
    fixture.records.head.bundle.timestamp = parseJson(fixture.set.sign(timestamp));
    const { bundle } = fixture.records.head;
    // Establish a genuinely signed complete public graph before it expires.
    verifyPublicChain({ trustedRoot: fixture.records.root.body, product: 'pinvou', component: 'app', now: NOW + 1,
      timestampBytes: canonicalize(bundle.timestamp), snapshotBytes: canonicalize(bundle.snapshot),
      members: bundle.members.map(canonicalize) });
    const now = timestamp.expiresAt;
    const command = metadataRotation(fixture, role, now);
    assert.throws(() => planPublishRoot(fixture.records, command, now), { code: 'SIGNING_KEY_NOT_AUTHORIZED' });
  }
});

test('private unexpired Release and expiry-free Package remain required after public metadata expires', () => {
  const fixture = publicationFixture();
  const now = NOW + 90 * DAY;
  const privateRelease = structuredClone(fixture.candidateReleaseEnvelope.signed);
  privateRelease.issuedAt++; privateRelease.expiresAt++;
  fixture.records.head.releaseEnvelopes['release-3'] = parseJson(fixture.set.sign(privateRelease));
  fixture.records['release-3'].envelope = fixture.records.head.releaseEnvelopes['release-3'];
  fixture.records['release-3'].envelopeSha256 = objectHash(fixture.records['release-3'].envelope);
  const releaseCommand = metadataRotation(fixture, 'release', now);
  assert.throws(() => planPublishRoot(fixture.records, releaseCommand, now), { code: 'SIGNING_KEY_NOT_AUTHORIZED' });

  const packageFixture = publicationFixture();
  const packageCommand = metadataRotation(packageFixture, 'package', now + 1);
  assert.throws(() => planPublishRoot(packageFixture.records, packageCommand, now + 1), { code: 'SIGNING_KEY_NOT_AUTHORIZED' });
});

test('expired committed objects still require closed shape, original window and head context', () => {
  for (const [mutate, code] of [
    [(head) => { head.bundle.timestamp.signed.expiresAt = 'expired'; }, 'SCHEMA_INVALID'],
    [(head) => { head.bundle.timestamp.signed.expiresAt = head.bundle.timestamp.signed.issuedAt; }, 'TIME_WINDOW_INVALID'],
    [(head) => { head.bundle.timestamp.signed.component = 'other'; }, 'MODEL_CURRENT_METADATA_INVALID'],
    [(head) => { head.bundle.timestamp.signed.role = 'release'; }, 'MODEL_CURRENT_METADATA_INVALID'],
  ]) {
    const fixture = publicationFixture(); const now = NOW + 90 * DAY;
    const command = metadataRotation(fixture, null, now);
    mutate(fixture.records.head);
    assert.throws(() => planPublishRoot(fixture.records, command, now), { code });
  }
});

test('a later Root rotation does not reauthenticate expired history with a removed policy key', () => {
  const fixture = publicationFixture(); const now = NOW + DAY;
  const first = metadataRotation(fixture, 'timestamp', now);
  fixture.records = applyAtomically(fixture.records, planPublishRoot(fixture.records, first, now), now).records;
  const head = structuredClone(fixture.records.head);
  const second = metadataRotation(fixture, 'snapshot', now + 1);
  const result = applyAtomically(fixture.records, planPublishRoot(fixture.records, second, now + 1), now + 1).records;
  assert.equal(result.root.body.version, 3);
  assert.deepEqual(result.head, head);
});

test('expired fields, signatures and complete heads remain protected by publication CAS', () => {
  const fixture = publicationFixture(); const now = NOW + 90 * DAY;
  const command = metadataRotation(fixture, 'timestamp', now);
  const plan = planPublishRoot(fixture.records, command, now);
  for (const mutate of [
    (head) => { head.bundle.timestamp.signed.expiresAt++; },
    (head) => { head.bundle.timestamp.signatures[0].signature = 'A'.repeat(86); },
    (head) => { head.releaseEnvelopes['release-3'].signed.expiresAt++; },
    (head) => { head.published = false; },
    (head) => { head.revision++; },
  ]) {
    const raced = structuredClone(fixture.records); mutate(raced.head);
    assert.throws(() => applyAtomically(raced, plan, now), { code: 'MODEL_CAS_CONFLICT' });
  }
});

function privatePackageFixture() {
  const fixture = publicationFixture(); const { records, set } = fixture;
  // Both keys are genuinely authorized by the old Root. Public Package keeps
  // key 3; only the private-current Package needs key 0 after rotation.
  records.root.body.roles.find((policy) => policy.role === 'package').keyIds =
    [set.descriptors[3].keyId, set.descriptors[0].keyId];
  const packageEnvelope = parseJson(signEnvelope(fixture.candidatePackageEnvelope.signed, [set.keys[0]]));
  const release = structuredClone(fixture.candidateReleaseEnvelope.signed);
  release.targets[0].packageManifest = envelopeReference(canonicalize(packageEnvelope));
  const releaseEnvelope = parseJson(set.sign(release));
  records.head.releaseEnvelopes['release-3'] = releaseEnvelope;
  Object.assign(records['release-3'], { envelope: releaseEnvelope, envelopeSha256: objectHash(releaseEnvelope),
    frozenBusinessSha256: releaseBusinessHash(release) });
  records['target-3'].packageEnvelope = packageEnvelope;
  verifyEnvelope(canonicalize(packageEnvelope), { trustedRoot: records.root.body, now: NOW + 1,
    expected: { role: 'package', product: 'pinvou', component: 'app', scope: packageEnvelope.signed.scope } });
  return fixture;
}

test('Root must retain private-current Package keys even when the public Package remains compatible', () => {
  const fixture = privatePackageFixture(); const now = NOW + 90 * DAY + 1;
  const command = metadataRotation(fixture, null, now);
  const successor = parseJson(command.successorBytes).signed;
  successor.roles.find((policy) => policy.role === 'package').keyIds = [fixture.set.descriptors[3].keyId];
  command.successorBytes = fixture.set.sign(successor); approveKeyCommand(fixture.records, command, now);
  verifyEnvelope(fixture.set.bytes.package, { trustedRoot: successor, now,
    expected: { role: 'package', product: 'pinvou', component: 'app', scope: fixture.set.metadata.package.scope } });
  assert.throws(() => planPublishRoot(fixture.records, command, now), { code: 'SIGNING_KEY_NOT_AUTHORIZED' });
});

test('private-current Package resolution rejects absent, ambiguous and incorrectly bound protected sources', () => {
  for (const mutate of [
    (records) => { delete records.inventory; },
    (records) => { records.otherInventory = structuredClone(records.inventory); },
    (records) => { records.inventory.requiredEntityKeys = records.inventory.requiredEntityKeys.filter((key) => key !== 'target-3'); },
    (records) => { records.otherTarget = structuredClone(records['target-3']); records.inventory.requiredEntityKeys.push('otherTarget'); },
    (records) => { delete records['target-3'].packageEnvelope; },
    (records) => { records['target-3'].packageEnvelope = records['target-2'].packageEnvelope; },
    (records) => { records['target-3'].releaseKey = 'release-2'; },
    (records) => { records['target-3'].revision++; },
  ]) {
    const fixture = privatePackageFixture(); const now = NOW + 90 * DAY + 1;
    const command = metadataRotation(fixture, null, now); mutate(fixture.records);
    assert.throws(() => planPublishRoot(fixture.records, command, now), { code: 'MODEL_CURRENT_PACKAGE_INCOMPLETE' });
  }
});

test('private Package full references do not replace product, component or Release target bindings', () => {
  for (const [mutate, code] of [
    [(claims) => { claims.product = 'other'; }, 'MODEL_CURRENT_PACKAGE_INCOMPLETE'],
    [(claims) => { claims.component = 'engine'; }, 'MODEL_CURRENT_PACKAGE_INCOMPLETE'],
    [(claims) => { claims.scope.targetKey = 'linux-arm64-deb-perMachine';
      for (const helper of claims.helpers) helper.targetKey = claims.scope.targetKey; }, 'TARGET_BINDING_INVALID'],
    [(claims) => { claims.launcher.identity = 'different-launcher'; }, 'MODEL_CURRENT_PACKAGE_INCOMPLETE'],
    [(claims) => { claims.activationModes = ['stagedRestart']; }, 'MODEL_CURRENT_PACKAGE_INCOMPLETE'],
  ]) {
    const fixture = privatePackageFixture(); const { records, set } = fixture;
    const claims = structuredClone(records['target-3'].packageEnvelope.signed); mutate(claims);
    const item = parseJson(signEnvelope(claims, [set.keys[0]]));
    const release = structuredClone(records.head.releaseEnvelopes['release-3'].signed);
    release.targets[0].packageManifest = envelopeReference(canonicalize(item));
    const envelope = parseJson(set.sign(release));
    records.head.releaseEnvelopes['release-3'] = envelope;
    Object.assign(records['release-3'], { envelope, envelopeSha256: objectHash(envelope),
      frozenBusinessSha256: releaseBusinessHash(release) });
    records['target-3'].packageEnvelope = item;
    const now = NOW + 90 * DAY + 1; const command = metadataRotation(fixture, null, now);
    assert.throws(() => planPublishRoot(records, command, now), { code });
  }
});

test('private Package, release and complete owner inventory join CAS without entering the public Snapshot', () => {
  const fixture = privatePackageFixture(); const now = NOW + 90 * DAY + 1;
  const command = metadataRotation(fixture, null, now); const { records } = fixture;
  const plan = planPublishRoot(records, command, now);
  for (const key of ['inventory', 'release-3', 'target-3'])
    assert.ok(plan.readSet.some((read) => read.key === key && read.sha256 === captureRecord(records, key).sha256));
  const result = applyAtomically(records, plan, now).records;
  assert.deepEqual(result.head.bundle, structuredClone(records.head.bundle));
  assert.equal(result.head.bundle.members.some((item) => objectHash(item) === objectHash(records['target-3'].packageEnvelope)), false);
  for (const mutate of [
    (copy) => { copy['target-3'].packageEnvelope.signatures[0].signature = 'A'.repeat(86); },
    (copy) => { copy.inventory.requiredEntityKeys.pop(); },
    (copy) => { copy['release-3'].frozenBusinessSha256 = 'b'.repeat(64); },
  ]) {
    const raced = structuredClone(records); mutate(raced);
    assert.throws(() => applyAtomically(raced, plan, now), { code: 'MODEL_CAS_CONFLICT' });
  }
});
