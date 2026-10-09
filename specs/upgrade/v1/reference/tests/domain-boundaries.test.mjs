import test from 'node:test';
import assert from 'node:assert/strict';
import { applyAtomically, captureRecord } from '../models/atomic.mjs';
import { planBaselineCommand } from '../models/deployment.mjs';
import { planReviewReleaseTarget } from '../models/target-review.mjs';
import { approvalContext } from '../models/entities.mjs';
import { planRebuildFirst, planScheduleFirst, planActivateFirst } from '../models/first-activation.mjs';
import { planPublishRoot, planPublish } from '../models/publication.mjs';
import { objectHash } from '../models/inputs.mjs';
import { parseJson, canonicalize } from '../canonical-json.mjs';
import { envelopeReference } from '../signatures.mjs';
import { initialLineagePort, approveKeyCommand, publicationFixture, proposedPublication, refreshPaths, prepareResume } from './publication-fixtures.mjs';
import { operation, operationCommand } from './model-fixtures.mjs';
import { createFixtureSet, NOW, TARGET } from './fixtures.mjs';
import { DAY } from '../semantics.mjs';

test('baseline pause/resume keeps completed child immutable and never compares against itself as a candidate', () => {
  for (const completed of [false, true]) {
    const fixture = publicationFixture(); let records = fixture.records;
    if (completed) { records.baseline.rolloutKey = 'completed-child'; records['completed-child'] = { revision: 1, state: 'completed', deploymentKey: 'baseline' }; }
    const pause = { ...operationCommand, reason: 'pause-investigation', scopeKey: 'scope', deploymentKey: 'baseline', action: 'pause', expectedRevision: 1,
      opening: null, publication: proposedPublication(fixture, records) };
    records = applyAtomically(records, planBaselineCommand(records, pause, NOW + 1), NOW + 1).records;
    records.operation = operation(NOW + 2); refreshPaths(records, { deploymentKey: 'baseline', now: NOW + 2 });
    const resume = { ...pause, ...prepareResume(records, 'baseline'), action: 'resume', expectedRevision: 2, pathKey: 'path',
      publication: proposedPublication(fixture, records, { now: NOW + 2 }) };
    const active = applyAtomically(records, planBaselineCommand(records, resume, NOW + 2), NOW + 2).records;
    assert.equal(active.baseline.state, 'active'); assert.equal(active.scope.baselineDeploymentKey, 'baseline');
    if (completed) assert.equal(active['completed-child'].revision, 1);
    const noApproval = structuredClone(records); delete noApproval['resume-approval'];
    assert.throws(() => planBaselineCommand(noApproval, resume, NOW + 2));
    const unresolved = structuredClone(records); unresolved['pause-resolution'].state = 'pending';
    assert.throws(() => planBaselineCommand(unresolved, resume, NOW + 2), { code: 'MODEL_PAUSE_UNRESOLVED' });
    const revoked = structuredClone(records); revoked['target-2'].state = 'revoked';
    assert.throws(() => planBaselineCommand(revoked, resume, NOW + 2));
  }
});
test('Release Target approval requires frozen assembled parent, valid artifacts, exact supply and mode certification', () => {
  const fixture = publicationFixture(); const records = fixture.records;
  const target = records['target-2']; target.state = 'in_review'; target.targetKey = TARGET;
  target.content = { packageManifest: envelopeReference(canonicalize(target.packageEnvelope)), activationModes: ['directInstall', 'stagedRestart'],
    migrationMode: 'none', backupPolicy: 'notRequired' }; target.frozenContentSha256 = objectHash(target.content);
  const parent = records['release-2']; parent.state = 'assembled'; parent.product = 'pinvou'; parent.component = 'app';
  parent.content = { appVersion: '2.0.0', targets: [{ targetKey: 'target-2', contentSha256: target.frozenContentSha256 }] };
  parent.frozenContentSha256 = objectHash(parent.content);
  records['artifact-2'].packageEnvelopeSha256 = objectHash(target.packageEnvelope);
  records.reviewSupply = { revision: 1, state: 'approved', targetContentSha256: target.frozenContentSha256, effectiveExpiresAt: null };
  records.certificate = { revision: 1, projectionOwner: 'T46', state: 'passed', targetContentSha256: target.frozenContentSha256,
    activationModes: ['directInstall', 'stagedRestart'] };
  records.approval = { revision: 1, projectionOwner: 'T05', state: 'approved', authorId: 'author', reviewerIds: ['reviewer'],
    context: approvalContext('ApproveReleaseTarget', { product: 'pinvou', component: 'app', targetKey: TARGET }, 'target-2', 1),
    bodySha256: objectHash({ command: 'ApproveReleaseTarget', targetKey: 'target-2', targetRevision: 1, releaseRevision: 1,
      contentSha256: target.frozenContentSha256, certificationSha256: objectHash(records.certificate), supplySha256: objectHash(records.reviewSupply) }) };
  const command = { ...operationCommand, targetKey: 'target-2', expectedRevision: 1, toState: 'approved', rootKey: 'root', denyKey: 'deny',
    supplyKey: 'reviewSupply', certificationKey: 'certificate', approvalKey: 'approval' };
  const plan = planReviewReleaseTarget(records, command, NOW + 1);
  assert.equal(applyAtomically(records, plan, NOW + 1).records['target-2'].state, 'approved');
  for (const change of [ (copy) => { copy['release-2'].state = 'cancelled'; }, (copy) => { copy['artifact-2'].state = 'quarantined'; },
    (copy) => { copy.reviewSupply.effectiveExpiresAt = NOW + 1; }, (copy) => { copy.certificate.activationModes.pop(); },
    (copy) => { copy.approval.context.objectRevision = 2; } ]) {
    const changed = structuredClone(records); change(changed); assert.throws(() => planReviewReleaseTarget(changed, command, NOW + 1));
  }
  const changed = structuredClone(records); changed['release-2'].revision++;
  assert.throws(() => applyAtomically(changed, plan, NOW + 1), { code: 'MODEL_CAS_CONFLICT' });
});
test('stale first-activation sets can be replaced without reversing scheduled Deployment or extending old cleanup', () => {
  const fixture = publicationFixture(); const records = fixture.records;
  Object.assign(records.scope, { state: 'unactivated', selectionGeneration: 0, baselineDeploymentKey: null });
  Object.assign(records.head, { published: false, revision: 0, bundle: null }); records.baseline.state = 'in_review';
  refreshPaths(records, { deploymentKey: 'baseline', anticipatedRevision: 2, now: NOW + 1 });
  const schedule = { ...operationCommand, scopeKey: 'scope', deploymentKey: 'baseline', expectedDeploymentRevision: 1,
    rootKey: 'root', headKey: 'head', denyKey: 'deny', inventoryKey: 'inventory', stagedSetKey: 'old-set', pathKey: 'path',
    bundle: proposedPublication(fixture, records, { now: NOW + 1 }).bundle };
  const staged = applyAtomically(records, planScheduleFirst(records, schedule, NOW + 1), NOW + 1).records;
  const now = NOW + 900_001; staged.operation = operation(now); staged.root.revision++;
  refreshPaths(staged, { deploymentKey: 'baseline', now });
  const rebuild = { ...schedule, expectedDeploymentRevision: 2, replacesStagedSetKey: 'old-set', expectedStagedRevision: 1,
    stagedSetKey: 'new-set', bundle: proposedPublication(fixture, staged, { now }).bundle };
  const result = applyAtomically(staged, planRebuildFirst(staged, rebuild, now), now).records;
  assert.equal(result.baseline.revision, 2); assert.equal(result['old-set'].state, 'replaced');
  assert.equal(result['old-set'].cleanupAt, now + DAY); assert.equal(result['new-set'].expiresAt, now + 900_000);
  const activate = { ...operationCommand, scopeKey: 'scope', deploymentKey: 'baseline', stagedSetKey: 'old-set', pathKey: 'path',
    publication: proposedPublication(fixture, result, { now }) };
  assert.throws(() => planActivateFirst(result, activate, now));
});
test('Root publication discovers every active component and preserves its complete verification chain', () => {
  const fixture = publicationFixture(); const records = fixture.records; const engine = createFixtureSet({ component: 'engine' });
  records.root.body.keys.push(engine.descriptors[3]); records.root.body.roles.push(...engine.metadata.root.roles.filter((policy) => policy.role !== 'root'));
  records.engineHead = { revision: 1, recordKind: 'metadataHead', product: 'pinvou', component: 'engine', published: true,
    releaseEnvelopes: { [engine.metadata.release.scope.releaseId]: parseJson(engine.bytes.release) },
    bundle: { timestamp: parseJson(engine.bytes.timestamp), snapshot: parseJson(engine.bytes.snapshot),
      members: ['target', 'release', 'package'].map((role) => parseJson(engine.bytes[role])) } };
  records.components = { revision: 1, componentHeadKeys: ['head', 'engineHead'] };
  const successor = { ...structuredClone(records.root.body), version: 2, issuedAt: NOW + 1 };
  const command = { ...operationCommand, rootKey: 'root', activeComponentsKey: 'components', baseRoot: captureRecord(records, 'root'), successorBytes: fixture.set.sign(successor) };
  approveKeyCommand(records, command, NOW + 1);
  const plan = planPublishRoot(records, command, NOW + 1);
  assert.equal(applyAtomically(records, plan, NOW + 1).records.root.body.version, 2);
  const missing = structuredClone(records); missing.components.componentHeadKeys.pop(); assert.throws(() => planPublishRoot(missing, command, NOW + 1));
  const raced = structuredClone(records); raced.engineHead.revision++; assert.throws(() => applyAtomically(raced, plan, NOW + 1));
  successor.roles = successor.roles.filter((policy) => policy.component !== 'engine'); command.successorBytes = fixture.set.sign(successor);
  assert.throws(() => planPublishRoot(records, command, NOW + 1));
});
test('standalone publication cannot select a new baseline without its domain write set', () => {
  const fixture = publicationFixture(); const command = { ...operationCommand, publication: proposedPublication(fixture, fixture.records, { complete: true }) };
  assert.throws(() => planPublish(fixture.records, command, NOW + 1), { code: 'MODEL_DOMAIN_COMPOSITION_REQUIRED' });
});
test('a second scope stages and activates against a complete already-published component head', () => {
  const fixture = publicationFixture(); const records = fixture.records;
  records.root.body.roles.push({ ...records.root.body.roles.find((role) => role.role === 'target'), channel: 'beta' });
  initialLineagePort(records);
  records.betaScope = { ...structuredClone(records.scope), state: 'unactivated', channel: 'beta', selectionGeneration: 0,
    baselineDeploymentKey: null, runningRolloutKey: null, rolloutKeys: [] };
  records.betaSupply = { ...structuredClone(records['supply-2']), channel: 'beta' };
  records.betaDeployment = { ...structuredClone(records.baseline), state: 'in_review', channel: 'beta', deploymentId: 'beta-deployment',
    chain: { ...records.baseline.chain, deploymentKey: 'betaDeployment', supplyChainKey: 'betaSupply' } };
  records.inventory.scopeKeys.push('betaScope'); records.inventory.requiredEntityKeys.push('betaDeployment', 'betaSupply');
  refreshPaths(records, { deploymentKey: 'baseline' });
  const betaPath = structuredClone(records.path); betaPath.deploymentKey = 'betaDeployment'; betaPath.deploymentRevision = 2; betaPath.scopeKey = 'betaScope';
  betaPath.sources[0].steps[0].chain = records.betaDeployment.chain;
  const future = { ...records, betaDeployment: { ...records.betaDeployment, state: 'scheduled', revision: 2 } };
  const pathKeys = ['root', 'head', 'deny', 'registry', 'betaScope', 'betaDeployment', 'betaSupply', 'release-2', 'target-2', 'artifact-2',
    betaPath.sources[0].steps[0].certificationKey];
  betaPath.readSet = pathKeys.map((key) => ({ ...captureRecord(future, key), recordKind: future[key].recordKind })); records.betaPath = betaPath;
  const stableTarget = records.head.bundle.members.find((item) => item.signed.role === 'target');
  const betaTarget = structuredClone(fixture.set.metadata.target); betaTarget.scope.channel = 'beta';
  const members = [stableTarget, parseJson(fixture.set.sign(betaTarget)), records['release-2'].envelope, parseJson(fixture.set.bytes.package)];
  const snapshot = { ...structuredClone(fixture.set.metadata.snapshot), version: 2,
    entries: members.map((item) => envelopeReference(canonicalize(item))) };
  const signedSnapshot = parseJson(fixture.set.sign(snapshot));
  const timestamp = { ...structuredClone(fixture.set.metadata.timestamp), version: 2, snapshot: envelopeReference(canonicalize(signedSnapshot)) };
  const bundle = { timestamp: parseJson(fixture.set.sign(timestamp)), snapshot: signedSnapshot, members };
  const schedule = { ...operationCommand, scopeKey: 'betaScope', deploymentKey: 'betaDeployment', expectedDeploymentRevision: 1,
    rootKey: 'root', headKey: 'head', denyKey: 'deny', inventoryKey: 'inventory', stagedSetKey: 'betaSet', pathKey: 'betaPath', bundle };
  const staged = applyAtomically(records, planScheduleFirst(records, schedule, NOW + 1), NOW + 1).records;
  staged.operation = operation(NOW + 2);
  const publication = { rootKey: 'root', headKey: 'head', denyKey: 'deny', inventoryKey: 'inventory', mode: 'selection',
    affectedScopeKeys: ['betaScope'], baseRoot: captureRecord(staged, 'root'), baseHead: captureRecord(staged, 'head'),
    readSet: ['root', 'head', 'inventory', 'scope', 'betaScope', ...staged.inventory.requiredEntityKeys].map((key) => captureRecord(staged, key)), bundle };
  const activated = applyAtomically(staged, planActivateFirst(staged, { ...operationCommand, scopeKey: 'betaScope', deploymentKey: 'betaDeployment',
    stagedSetKey: 'betaSet', pathKey: 'betaPath', publication }, NOW + 2), NOW + 2).records;
  assert.equal(activated.betaScope.baselineDeploymentKey, 'betaDeployment'); assert.equal(activated.betaScope.selectionGeneration, 1);
  assert.equal(activated.scope.selectionGeneration, 1); assert.equal(activated.head.bundle.members.filter((item) => item.signed.role === 'target').length, 2);
});
