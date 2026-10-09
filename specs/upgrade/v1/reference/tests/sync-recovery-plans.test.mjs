import test from 'node:test';
import assert from 'node:assert/strict';
import { planSynchronizeStable } from '../models/stable-sync.mjs';
import { freezeStagePlan } from '../models/plans.mjs';
import { planReleaseQualityFreeze } from '../models/recovery.mjs';
import { planRolloutCommand } from '../models/rollout.mjs';
import { applyAtomically } from '../models/atomic.mjs';
import { objectHash } from '../models/inputs.mjs';
import { publicationFixture, rolloutCommand, openingFor, proposedPublication, catchUpQuality, refreshPaths } from './publication-fixtures.mjs';
import { operation, operationCommand } from './model-fixtures.mjs';
import { NOW, HASH, TARGET } from './fixtures.mjs';
import { DAY } from '../semantics.mjs';
import { approvalContext } from '../models/entities.mjs';

function wirePlan() {
  const threshold = { numerator: 1, denominator: 10 };
  const stage = (stageId, percentage) => ({ stageId, percentage, minimumObservationMs: 1000, minimumSamples: 10,
    thresholds: { download: threshold, verification: threshold, installation: threshold, health: threshold,
      preinstallation: null, activation: null } });
  return { stages: [stage('first', 10), stage('final', 100)], snIncludeSetRef: null, snExcludeSetRef: null,
    progressReportIntervalMs: 30_000, lossIntervalMs: 120_000 };
}
test('wire plan freezes exact metrics, sample policies and SN references without shape drift', () => {
  const plan = wirePlan(); const frozen = freezeStagePlan(plan, 'normal');
  assert.equal(frozen.stages[0].metrics.download.minimumSamples, 10);
  assert.deepEqual(frozen.stages[0].requiredMetrics, ['download', 'verification', 'installation', 'health']);
  assert.throws(() => freezeStagePlan(plan, 'silent'));
  for (const stage of plan.stages) { stage.thresholds.preinstallation = { numerator: 1, denominator: 20 };
    stage.thresholds.activation = { numerator: 1, denominator: 20 }; }
  assert.equal(freezeStagePlan(plan, 'silent').stages[0].requiredMetrics.length, 6);
  const bad = wirePlan(); bad.stages[1].percentage = 10; assert.throws(() => freezeStagePlan(bad, 'normal'));
  bad.stages[1].percentage = 99; assert.throws(() => freezeStagePlan(bad, 'normal'));
  bad.stages[1].percentage = 100; bad.stages[0].thresholds.download.numerator = 11;
  assert.throws(() => freezeStagePlan(bad, 'normal'));
});

function syncFixture() {
  const fixture = publicationFixture(); const records = fixture.records;
  records.syncIndex = { revision: 1, bySyncKey: {}, byTarget: {} };
  const configuration = { releaseId: 'release-2', releaseRevision: 1, releaseTargetId: records['target-2'].releaseTargetId,
    releaseTargetRevision: 1, upgradeType: 'normal', releaseVisibleAt: NOW, installNotBefore: NOW,
    installNotAfter: null, plan: wirePlan(), responsibilityIdentity: 'operator-1' };
  const selections = ['beta', 'internal'].map((channel) => ({ channel, targetKey: TARGET,
    sourceStableDeploymentId: records.baseline.deploymentId, sourceStableDeploymentRevision: 1, configuration }));
  const request = { protocolVersion: 1, commandKey: 'sync-1', reason: 'explicit-sync',
    scope: { product: 'pinvou', component: 'app', channel: null, targetKey: null }, sourceReleaseId: 'release-2',
    expectedReadSet: [{ recordKind: 'deployment', recordId: records.baseline.deploymentId, revision: 1, sha256: objectHash(records.baseline) }], selections };
  records.permission = { revision: 1, product: 'pinvou', component: 'app', operatorId: 'operator-1', state: 'active', expiresAt: NOW + DAY,
    grants: selections.map(({ channel, targetKey }) => ({ channel, targetKey })) };
  const items = selections.map(({ channel }) => {
    records[`${channel}-scope`] = { revision: 1, recordKind: 'selectionScope', product: 'pinvou', component: 'app',
      state: 'unactivated', channel, targetKey: TARGET, baselineDeploymentKey: null, rolloutKeys: [], deploymentIndexKey: `${channel}-deployment-index` };
    records[`${channel}-deployment-index`] = { revision: 1, scopeKey: `${channel}-scope`, memberKeys: [] };
    return { sourceChain: records.baseline.chain, sourceScopeKey: 'scope', targetScopeKey: `${channel}-scope`, draftKey: `${channel}-draft`, rolloutKey: `${channel}-rollout` };
  });
  const requestDigest = objectHash({ request, operatorId: 'operator-1' }); records.operation.requestDigest = requestDigest;
  return { records, command: { ...operationCommand, operatorId: 'operator-1', requestDigest, request,
    syncIndexKey: 'syncIndex', permissionKey: 'permission', sourceReleaseKey: 'release-2', items, batchKey: 'batch', batchId: 'batch-1' } };
}
test('Stable sync creates only explicit independent drafts and permanent replay requires current scoped identity', () => {
  const { records, command } = syncFixture(); const result = applyAtomically(records, planSynchronizeStable(records, command, NOW + 1), NOW + 1).records;
  assert.equal(result['beta-draft'].state, 'draft'); assert.equal(result['internal-rollout'].state, 'draft');
  assert.equal(result['beta-draft'].approvalKey, null); assert.equal(result['beta-draft'].supplyChainKey, null);
  assert.equal(result.scope.selectionGeneration, 1); assert.equal(result.head.revision, 1);
  result.permission.expiresAt = NOW + 50 * DAY;
  const replay = applyAtomically(result, planSynchronizeStable(result, command, NOW + 40 * DAY), NOW + 40 * DAY).records;
  assert.equal(replay.batch.revision, 1); assert.equal(replay['beta-draft'].revision, 1);
  for (const field of ['operatorId', 'product', 'component']) {
    const denied = structuredClone(result); denied.permission[field] = 'other';
    assert.throws(() => planSynchronizeStable(denied, command, NOW + 40 * DAY), { code: 'MODEL_SYNC_PERMISSION_INVALID' });
  }
  const denied = structuredClone(result); denied.permission.grants.pop(); assert.throws(() => planSynchronizeStable(denied, command, NOW + 40 * DAY));
});
test('Stable sync source/target races, partial permissions and duplicate targets yield no writes', () => {
  const { records, command } = syncFixture(); const plan = planSynchronizeStable(records, command, NOW + 1);
  for (const key of ['permission', 'baseline', 'supply-2', 'release-2', 'target-2', 'beta-scope', 'internal-scope', 'syncIndex']) {
    const changed = structuredClone(records); changed[key].revision++;
    assert.throws(() => applyAtomically(changed, plan, NOW + 1), { code: 'MODEL_CAS_CONFLICT' });
  }
  const paused = structuredClone(records); paused.baseline.state = 'paused'; assert.throws(() => planSynchronizeStable(paused, command, NOW + 1));
  const expired = structuredClone(records); expired['supply-2'].effectiveExpiresAt = NOW + 1; assert.throws(() => planSynchronizeStable(expired, command, NOW + 1));
  const denied = structuredClone(records); denied.permission.grants.pop(); assert.throws(() => planSynchronizeStable(denied, command, NOW + 1));
  const duplicate = structuredClone(records); duplicate.existing = { revision: 1, recordKind: 'deployment', product: 'pinvou', component: 'app',
    channel: 'beta', targetKey: TARGET, appVersion: '2.0.0', releaseTargetKey: 'another-target' };
  duplicate['beta-deployment-index'].memberKeys = ['existing'];
  assert.throws(() => planSynchronizeStable(duplicate, command, NOW + 1), { code: 'MODEL_SYNC_DUPLICATE' });
  const committed = applyAtomically(records, plan, NOW + 1).records;
  const other = structuredClone(command); other.request.commandKey = 'sync-2'; other.requestDigest = objectHash({ request: other.request, operatorId: other.operatorId });
  committed.operation = operation(NOW + 2); committed.operation.requestDigest = other.requestDigest;
  assert.throws(() => planSynchronizeStable(committed, other, NOW + 2), { code: 'MODEL_SYNC_DUPLICATE' });
});

function recoveryFixture() {
  const fixture = publicationFixture(); let records = applyAtomically(fixture.records,
    planRolloutCommand(fixture.records, rolloutCommand(fixture, fixture.records, 'start'), NOW + 1), NOW + 1).records;
  records.rollout.qualityFrozen = true; records['contribution-head'].watermark = 7;
  records['contribution-head'].windowGroups['review-window'] = [HASH];
  const originalPlan = { ...structuredClone(records.plan.body.stages[0]), minimumObservationMs: 1500 };
  const observation = { startedAt: NOW + 2, groups: [{ groupIdentity: HASH,
    metrics: Object.fromEntries(originalPlan.requiredMetrics.map((metric) => [metric, { succeeded: 10, failed: 0, incomplete: 0, unknown: 0 }])) }] };
  records.reason = { revision: 1, state: 'frozen', originalPlan, originalGroupIdentities: [HASH] };
  records['freeze-index'] = { revision: 1, reasonKeys: ['reason'] };
  records.review = { revision: 1, state: 'observing', rolloutKey: 'rollout', freezeIndexRevision: 1,
    planSha256: records.rollout.planSha256, stageFactRevision: records['stage-fact'].revision, reasonKeys: ['reason'], binding: {},
    windowId: 'review-window', windowStart: NOW + 2, windowEnd: NOW + DAY, sourceHeadKey: 'contribution-head',
    sourceWatermark: 7, processedWatermark: 7, targetIdentitySha256: HASH,
    groups: [{ reasonKey: 'reason', originalPlan, observation }], currentStage: { plan: records.plan.body.stages[0], observation } };
  records.approval = { revision: 1, projectionOwner: 'T05', state: 'approved', authorId: 'author', reviewerIds: ['reviewer-a', 'reviewer-b'],
    context: approvalContext('ReleaseQualityFreeze', records.scope, 'review', 1),
    bodySha256: objectHash({ command: 'ReleaseQualityFreeze', reviewKey: 'review', review: records.review,
      rolloutKey: 'rollout', rolloutRevision: records.rollout.revision }) };
  const now = NOW + 2002; records.operation = operation(now);
  refreshPaths(records, { now });
  const opening = openingFor(records, 'advance');
  const command = { ...operationCommand, rolloutKey: 'rollout', freezeIndexKey: 'freeze-index', reviewKey: 'review', approvalKey: 'approval',
    newWindowId: 'post-review-window', opening, pathKey: 'path', publication: proposedPublication(fixture, records, { now, opening }) };
  return { fixture, records, command, now };
}
test('quality release checks all historical causes and final chains, then starts independent full observation', () => {
  const { fixture, records, command, now } = recoveryFixture();
  const plan = planReleaseQualityFreeze(records, command, now); const result = applyAtomically(records, plan, now).records;
  assert.equal(result.rollout.percentage, 10); assert.equal(result.rollout.qualityFrozen, false);
  assert.deepEqual(result.rollout.postRecoveryReasonKeys, ['reason']); assert.equal(result.reason.state, 'released');
  assert.equal(result['stage-fact'].sourceWatermark, 7); assert.equal(result['stage-fact'].effectiveAt, now);
  for (const key of ['reason', 'freeze-index', 'contribution-head', 'root', 'head', 'supply-3']) {
    const changed = structuredClone(records); changed[key].revision++;
    assert.throws(() => applyAtomically(changed, plan, now), { code: 'MODEL_CAS_CONFLICT' });
  }
  result.operation = operation(now + 1600); catchUpQuality(result);
  const advance = rolloutCommand(fixture, result, 'advance', now + 1600);
  assert.throws(() => planRolloutCommand(result, advance, now + 1600), { code: 'MODEL_QUALITY_INCOMPLETE' });
  result.quality.historicalObservations = [{ reasonKey: 'reason', observation: { ...records.review.groups[0].observation, startedAt: now } }];
  const advanced = applyAtomically(result, planRolloutCommand(result, advance, now + 1600), now + 1600).records;
  assert.equal(advanced.rollout.percentage, 100); assert.deepEqual(advanced.rollout.postRecoveryReasonKeys, []);
  assert.throws(() => planRolloutCommand(result, { ...advance, windowId: 'post-review-window' }, now + 1600));
});
