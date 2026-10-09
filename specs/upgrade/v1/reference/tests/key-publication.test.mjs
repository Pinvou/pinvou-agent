import test from 'node:test';
import assert from 'node:assert/strict';
import { applyAtomically, captureRecord } from '../models/atomic.mjs';
import { planPublishRoot, planEmergencyDeny } from '../models/publication.mjs';
import { verifyEnvelope } from '../signatures.mjs';
import { objectHash } from '../models/inputs.mjs';
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
