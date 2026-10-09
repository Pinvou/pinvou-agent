import test from 'node:test';
import assert from 'node:assert/strict';
import { parseJson, canonicalize } from '../canonical-json.mjs';
import { envelopeReference, signEnvelope } from '../signatures.mjs';
import { DAY } from '../semantics.mjs';
import { applyAtomically, captureRecord } from '../models/atomic.mjs';
import { planPublish, planPublishRoot, planEmergencyDeny } from '../models/publication.mjs';
import { chainReads } from '../models/entities.mjs';
import { currentReleaseHash, deniedKeys, deniedKeyOptions } from '../models/metadata-inputs.mjs';
import { objectHash } from '../models/inputs.mjs';
import { transactionTransition } from '../models/transaction.mjs';
import { planEvent } from '../models/events.mjs';
import { selectNextHop } from '../models/selection.mjs';
import { assertApiShape } from '../api-registry.mjs';
import { approveKeyCommand, publicationFixture, proposedPublication } from './publication-fixtures.mjs';
import { modelFacts, modelTransaction, operation, operationCommand } from './model-fixtures.mjs';
import { NOW, HASH, TARGET } from './fixtures.mjs';

function resignBundle(set, members, now, { timestampVersion = 2, snapshotVersion = 2 } = {}) {
  const snapshot = { ...structuredClone(set.metadata.snapshot), version: snapshotVersion,
    issuedAt: now, expiresAt: now + 7 * DAY, entries: members.map((item) => envelopeReference(canonicalize(item))) };
  const signedSnapshot = parseJson(set.sign(snapshot));
  const timestamp = { ...structuredClone(set.metadata.timestamp), version: timestampVersion,
    issuedAt: now, expiresAt: now + DAY, snapshot: envelopeReference(canonicalize(signedSnapshot)) };
  return { timestamp: parseJson(set.sign(timestamp)), snapshot: signedSnapshot, members };
}

test('expired Root and public metadata recover by rotation then renewal without changing Release business revision', () => {
  const fixture = publicationFixture(); const now = NOW + 366 * DAY; const records = fixture.records;
  records.components = { revision: 1, componentHeadKeys: ['head'] }; records.operation = operation(now);
  const successor = { ...structuredClone(records.root.body), version: 2, issuedAt: now, expiresAt: now + 365 * DAY };
  const rotate = { ...operationCommand, rootKey: 'root', activeComponentsKey: 'components',
    baseRoot: captureRecord(records, 'root'), successorBytes: fixture.set.sign(successor) };
  approveKeyCommand(records, rotate, now);
  let current = applyAtomically(records, planPublishRoot(records, rotate, now), now).records;
  assert.throws(() => chainReads(current, current.baseline.chain, now, { expectedScope: current.scope }), { code: 'TIME_WINDOW_CLOSED' });
  current.operation = operation(now + 1);
  const release = { ...structuredClone(fixture.set.metadata.release), revision: 2, issuedAt: now + 1, expiresAt: now + 1 + 90 * DAY };
  const signedRelease = parseJson(fixture.set.sign(release));
  const target = { ...structuredClone(fixture.set.metadata.target), version: 2, issuedAt: now + 1,
    expiresAt: now + 1 + 30 * DAY, baselineRelease: envelopeReference(canonicalize(signedRelease)) };
  const publication = proposedPublication(fixture, current, { now: now + 1, mode: 'refresh' });
  publication.bundle = resignBundle(fixture.set, [parseJson(fixture.set.sign(target)), signedRelease,
    parseJson(fixture.set.bytes.package)], now + 1);
  current = applyAtomically(current, planPublish(current, { ...operationCommand, publication }, now + 1), now + 1).records;
  assert(chainReads(current, current.baseline.chain, now + 1, { expectedScope: current.scope }).includes('head'));
  assert.equal(current['release-2'].revision, 1); assert.equal(current.scope.selectionGeneration, 1);
  assert.equal(currentReleaseHash(current, 'release-2'), objectHash(signedRelease));
});

test('Root genesis uses the independent protected anchor and cannot reset an existing current head', () => {
  const fixture = publicationFixture(); const records = fixture.records;
  Object.assign(records.root, { revision: 0, published: false }); Object.assign(records.head, { revision: 0, published: false, bundle: null });
  records.components = { revision: 1, componentHeadKeys: ['head'] };
  records.anchor = { revision: 1, recordKind: 'initialRootAnchor', state: 'provisioned', product: 'pinvou', bodySha256: objectHash(records.root.body) };
  const command = { ...operationCommand, rootKey: 'root', anchorKey: 'anchor', activeComponentsKey: 'components',
    baseRoot: captureRecord(records, 'root'), successorBytes: fixture.set.bytes.root };
  approveKeyCommand(records, command, NOW);
  const plan = planPublishRoot(records, command, NOW);
  const initialized = applyAtomically(records, plan, NOW).records;
  assert.equal(initialized.root.published, true); assert.equal(initialized.root.body.version, 1);
  initialized.operation = operation();
  assert.throws(() => planPublishRoot(initialized, { ...command, baseRoot: captureRecord(initialized, 'root') }, NOW));
  const changed = structuredClone(records); changed.anchor.bodySha256 = 'b'.repeat(64);
  assert.throws(() => planPublishRoot(changed, command, NOW));
  assert.throws(() => applyAtomically(changed, plan, NOW), { code: 'MODEL_CAS_CONFLICT' });
  assert.throws(() => chainReads(records, records.baseline.chain, NOW, { expectedScope: records.scope }), { code: 'MODEL_SCOPE_INVALID' });
});

test('EmergencyDeny reaches events, successful replay and metadata through the same purpose-scoped record', () => {
  const fixture = publicationFixture(); const records = fixture.records; const claims = fixture.set.claims['install-transaction-event'];
  records.transaction = modelTransaction(); records['contribution-head'].windowGroups['window-1'] = [HASH];
  const facts = { factType: 'local-state' };
  const event = { eventId: 'event-1', sequence: 1, occurredAt: NOW + 1, lineageId: 'transaction-1', executionPurpose: 'install',
    kind: 'authorization_consumed', fromState: 'authorization_consumed', toState: 'authorization_consumed',
    facts, factsSha256: objectHash(facts), targetIdentitySha256: HASH };
  const command = { rootKey: 'root', denyKey: 'deny', lineageKey: 'transaction', ledgerKey: 'ledger', outboxKey: 'outbox', event,
    credential: parseJson(fixture.set.sign(claims)), expectedContext: { role: claims.role, product: 'pinvou', component: 'app', scope: claims.scope },
    scope: { product: 'pinvou', component: 'app', installId: 'install-1', installationScopeId: 'scope-1',
      channel: 'stable', channelRevision: 1, targetKey: TARGET } };
  const accepted = applyAtomically(records, planEvent(records, command, NOW + 1), NOW + 1).records;
  for (const snapshot of [records, accepted]) {
    snapshot.operation = operation(NOW + 2); snapshot.denyInventory = { revision: 1, affectedScopeKeys: ['scope'], product: 'pinvou', component: 'app' };
    const denyCommand = { ...operationCommand, denyKey: 'deny', inventoryKey: 'denyInventory', affectedScopeKeys: ['scope'], jobKey: 'deny-job',
      denyEntries: [{ subjectKind: 'signingKey', subjectId: claims.signingKeyId, roles: [claims.role, 'release'] }] };
    approveKeyCommand(snapshot, denyCommand, NOW + 2);
    const denied = applyAtomically(snapshot, planEmergencyDeny(snapshot, denyCommand, NOW + 2), NOW + 2).records;
    assert.throws(() => planEvent(denied, command, NOW + 3), { code: 'EVENT_KEY_DENIED' });
    assert.throws(() => chainReads(denied, denied.baseline.chain, NOW + 3, { expectedScope: denied.scope }), { code: 'SIGNING_KEY_DENIED' });
    assert.deepEqual(deniedKeys(denied, 'deny', 'download'), []);
  }
  assert.throws(() => assertApiShape('deny-entry', { subjectKind: 'signingKey', subjectId: HASH, roles: ['constructor'] }));
  assert.equal(Object.getPrototypeOf(deniedKeyOptions(records, 'deny').deniedKeyIdsByRole), null);
});
test('same Release metadata revision with different valid signature bytes is a fork', () => {
  const fixture = publicationFixture(); const records = fixture.records;
  records.root.body.roles.find((policy) => policy.role === 'release').keyIds.push(fixture.set.descriptors[0].keyId);
  const release = parseJson(signEnvelope(records['release-2'].envelope.signed, [fixture.set.keys[0], fixture.set.keys[3]]));
  const target = { ...structuredClone(fixture.set.metadata.target), version: 2,
    baselineRelease: envelopeReference(canonicalize(release)) };
  const publication = proposedPublication(fixture, records, { mode: 'refresh' });
  publication.bundle = resignBundle(fixture.set, [parseJson(fixture.set.sign(target)), release, parseJson(fixture.set.bytes.package)], NOW + 1);
  assert.throws(() => planPublish(records, { ...operationCommand, publication }, NOW + 1), { code: 'MODEL_METADATA_FORK' });
});

test('health budget retains its first anchor and execution observations cannot precede the boundary', () => {
  const transaction = modelTransaction('installation_verified'); const facts = modelFacts();
  const health = transactionTransition(transaction, 'health_check_started', { now: NOW + 1, facts: { ...facts, healthStartedAt: NOW + 1 } });
  assert.equal(health.healthStartedAt, NOW + 1);
  assert.throws(() => transactionTransition(health, 'succeeded', { now: NOW + 600_000,
    facts: { ...facts, healthStartedAt: NOW + 599_999 } }), { code: 'MODEL_HEALTH_INVALID' });
  assert.equal(transactionTransition(health, 'succeeded', { now: NOW + 300_001,
    facts: { ...facts, healthStartedAt: NOW + 1 } }).state, 'succeeded');
  const backwards = modelTransaction('installer_started'); backwards.boundaryAt = NOW + 100;
  assert.throws(() => transactionTransition(backwards, 'reconciling', { now: NOW + 1, facts }), { code: 'MODEL_BOUNDARY_INVALID' });
});

test('legal long paths use iterative reachability and uncertified higher hops do not displace valid alternatives', () => {
  const template = { deploymentId: 'baseline', channel: 'stable', version: '3.0.0', minimumSourceVersion: '1.0.0',
    hopKind: 'ordinary', deploymentState: 'active', releaseState: 'closed', releaseTargetState: 'approved', artifactState: 'valid',
    supplyChainState: 'approved', supplyChainExpiresAt: null, releaseVisibleAt: NOW, installNotBefore: NOW, installNotAfter: null,
    isBaseline: true, ordinaryPathApproval: 'approved', ownRolloutState: null, bridgeState: null, releaseTargetId: 'target-1',
    releaseTargetRevision: 1, packageId: 'package-1', packageSha256: HASH, transformId: 'transform-1', transformRevision: 1,
    upgradeType: 'normal', activationMode: 'directInstall', certificationState: 'certified' };
  const input = { currentVersion: '0.0.0', supportFloorVersion: '0.0.0', channel: 'stable', now: NOW,
    stableFactsSha256: HASH, baselineId: 'd-2500', candidateId: null, rollout: null, nodes: [],
    profiles: [{ sourceProfileId: HASH, stableFactsSha256: HASH, state: 'selectable', policy: null }] };
  input.currentVersion = '0.0.0'; input.supportFloorVersion = '0.0.0'; input.baselineId = 'd-2500'; input.candidateId = null; input.rollout = null;
  input.nodes = Array.from({ length: 2500 }, (_, index) => ({ ...template, deploymentId: `d-${index + 1}`, version: `${index + 1}.0.0`,
    minimumSourceVersion: `${index}.0.0`, isBaseline: index === 2499, ordinaryPathApproval: 'approved' }));
  assert.equal(selectNextHop(input).hopId, 'd-1');
  input.nodes = [{ ...template, deploymentId: 'baseline', version: '3.0.0', minimumSourceVersion: '2.0.0', isBaseline: true },
    { ...template, deploymentId: 'low', version: '2.0.0', minimumSourceVersion: '1.0.0', isBaseline: false, ordinaryPathApproval: 'approved' },
    { ...template, deploymentId: 'high', version: '2.5.0', minimumSourceVersion: '1.0.0', isBaseline: false,
      ordinaryPathApproval: 'approved', certificationState: 'unverified' }];
  input.baselineId = 'baseline'; input.currentVersion = '1.0.0';
  assert.equal(selectNextHop(input).hopId, 'low');
});
