import test from 'node:test';
import assert from 'node:assert/strict';
import { applyAtomically, captureRecord } from '../models/atomic.mjs';
import { planPublish, planEmergencyDeny, planReconcileMetadata, planPublishRoot } from '../models/publication.mjs';
import { planRolloutCommand } from '../models/rollout.mjs';
import { planReleaseTransition, releaseCancellationBody, approvalContext } from '../models/entities.mjs';
import { objectHash } from '../models/inputs.mjs';
import { planScheduleFirst, planActivateFirst, stagedSetCleanupDue } from '../models/first-activation.mjs';
import { publicationFixture, proposedPublication, rolloutCommand, catchUpQuality, refreshPaths } from './publication-fixtures.mjs';
import { operation, operationCommand } from './model-fixtures.mjs';
import { NOW, HASH } from './fixtures.mjs';
import { rolloutCommitment } from '../digests.mjs';
import { DAY } from '../semantics.mjs';

test('publication compares both heads and complete inventory; refresh preserves generation', () => {
  const fixture = publicationFixture(); const { records } = fixture;
  const command = { ...operationCommand, publication: proposedPublication(fixture, records, { mode: 'refresh' }) };
  const plan = planPublish(records, command, NOW + 1); const result = applyAtomically(records, plan, NOW + 1).records;
  assert.equal(result.head.revision, 2); assert.equal(result.scope.selectionGeneration, 1);
  for (const key of ['root', 'head', 'scope', 'registry', 'supply-3']) {
    const changed = structuredClone(records); changed[key].revision += 1;
    assert.throws(() => applyAtomically(changed, plan, NOW + 1), { code: 'MODEL_CAS_CONFLICT' });
  }
  const incomplete = structuredClone(command); incomplete.publication.readSet = incomplete.publication.readSet.filter((read) => read.key !== 'registry');
  assert.throws(() => planPublish(records, incomplete, NOW + 1), { code: 'MODEL_READ_SET_INCOMPLETE' });
});
test('rollout starts first stage, advances only adjacent after complete evidence, and atomically switches baseline', () => {
  const fixture = publicationFixture(); let records = fixture.records;
  records = applyAtomically(records, planRolloutCommand(records, rolloutCommand(fixture, records, 'start'), NOW + 1), NOW + 1).records;
  assert.equal(records.rollout.percentage, 10); assert.equal(records.rollout.stageIndex, 0); assert.equal(records['stage-fact'].effectiveAt, NOW + 1);
  catchUpQuality(records); records.operation = operation(NOW + 1000);
  assert.throws(() => planRolloutCommand(records, rolloutCommand(fixture, records, 'advance', NOW + 1000), NOW + 1000), { code: 'QUALITY_INCOMPLETE' });
  records.operation = operation(NOW + 1001); const advance = rolloutCommand(fixture, records, 'advance', NOW + 1001);
  const prepared = planRolloutCommand(records, advance, NOW + 1001);
  const late = structuredClone(records); late['contribution-head'].revision += 1; late['contribution-head'].watermark += 1;
  assert.throws(() => applyAtomically(late, prepared, NOW + 1001), { code: 'MODEL_CAS_CONFLICT' });
  assert.throws(() => planRolloutCommand(late, rolloutCommand(fixture, late, 'advance', NOW + 1001), NOW + 1001), { code: 'MODEL_QUALITY_STALE' });
  records = applyAtomically(records, prepared, NOW + 1001).records;
  assert.equal(records.rollout.percentage, 100); assert.equal(records.rollout.stageIndex, 1);
  assert.notEqual(records['stage-fact'].windowId, 'window-2');
  assert.throws(() => planRolloutCommand(records, rolloutCommand(fixture, records, 'complete', NOW + 2001), NOW + 2001));
  catchUpQuality(records); records.operation = operation(NOW + 2001);
  records = applyAtomically(records, planRolloutCommand(records, rolloutCommand(fixture, records, 'complete', NOW + 2001), NOW + 2001), NOW + 2001).records;
  assert.equal(records.rollout.state, 'completed'); assert.equal(records.scope.baselineDeploymentKey, 'candidate');
  assert.equal(records.baseline.state, 'superseded'); assert.equal(records.scope.runningRolloutKey, null);
  const target = records.head.bundle.members.find((member) => member.signed.role === 'target');
  assert.equal(target.signed.rolloutSetCommitment, rolloutCommitment(null));
  assert.equal(records.scope.selectionGeneration, 4);
});
test('pause/resume preserves percentage and freeze; paused cannot complete or skip quality', () => {
  const fixture = publicationFixture(); let records = fixture.records;
  records = applyAtomically(records, planRolloutCommand(records, rolloutCommand(fixture, records, 'start'), NOW + 1), NOW + 1).records;
  records.operation = operation(NOW + 2);
  records = applyAtomically(records, planRolloutCommand(records, rolloutCommand(fixture, records, 'pause', NOW + 2), NOW + 2), NOW + 2).records;
  assert.equal(records.candidate.state, 'paused'); assert.equal(records.rollout.state, 'paused');
  records.operation = operation(NOW + 3); records.rollout.qualityFrozen = true;
  assert.throws(() => planRolloutCommand(records, rolloutCommand(fixture, records, 'complete', NOW + 3), NOW + 3));
  records = applyAtomically(records, planRolloutCommand(records, rolloutCommand(fixture, records, 'resume', NOW + 3), NOW + 3), NOW + 3).records;
  assert.equal(records.candidate.state, 'active'); assert.equal(records.rollout.percentage, 10); assert.equal(records.rollout.qualityFrozen, true);
  assert.equal(records['stage-fact'].effectiveAt, NOW + 3);
});
test('running plan, candidate version, supply expiry, actor and abort cause have mandatory guards', () => {
  for (const mutation of [
    (records) => { records.plan.body.stages[0].percentage = 20; },
    (records) => { records.candidate.appVersion = '2.0.0'; records['release-3'].appVersion = '2.0.0'; },
    (records) => { records['supply-3'].effectiveExpiresAt = NOW + 1; },
  ]) {
    const fixture = publicationFixture(); mutation(fixture.records);
    assert.throws(() => planRolloutCommand(fixture.records, rolloutCommand(fixture, fixture.records, 'start'), NOW + 1));
  }
  const fixture = publicationFixture(); const command = rolloutCommand(fixture, fixture.records, 'start'); command.actor = 'operator';
  assert.throws(() => planRolloutCommand(fixture.records, command, NOW + 1));
  const aborted = rolloutCommand(fixture, fixture.records, 'abort'); aborted.abortCause = 'operator_abort'; aborted.actor = 'operator';
  const result = applyAtomically(fixture.records, planRolloutCommand(fixture.records, aborted, NOW + 1), NOW + 1).records;
  assert.equal(result.rollout.abortCause, 'operator_abort'); assert.equal(result.candidate.state, 'withdrawn');
  assert.throws(() => planRolloutCommand(fixture.records, { ...aborted, abortCause: 'unknown' }, NOW + 1));
});
test('emergency deny and pending job commit together; only matching reconciliation clears pending', () => {
  const fixture = publicationFixture(); fixture.records.denyInventory = { revision: 1, product: 'pinvou', component: 'app', affectedScopeKeys: ['scope'] };
  const command = { ...operationCommand, denyKey: 'deny', inventoryKey: 'denyInventory',
    denyEntries: [{ subjectKind: 'artifact', subjectId: 'artifact-3', roles: [] }], affectedScopeKeys: ['scope'], jobKey: 'job' };
  let records = applyAtomically(fixture.records, planEmergencyDeny(fixture.records, command, NOW + 1), NOW + 1).records;
  assert.equal(records.scope.metadataSyncPending, true); assert.equal(records.job.state, 'pending'); assert.equal(records.scope.selectionGeneration, 2);
  records.operation = operation(NOW + 2);
  const reconcile = { ...operationCommand, jobKey: 'job', expectedJobRevision: 1, denyKey: 'deny',
    publication: proposedPublication(fixture, records, { mode: 'reconcile', now: NOW + 2 }) };
  const plan = planReconcileMetadata(records, reconcile, NOW + 2); const changed = structuredClone(records); changed.job.revision += 1;
  assert.throws(() => applyAtomically(changed, plan, NOW + 2));
  records = applyAtomically(records, plan, NOW + 2).records;
  assert.equal(records.scope.metadataSyncPending, false); assert.equal(records.scope.selectionGeneration, 2); assert.equal(records.job.state, 'completed');
});
test('release close/automatic cancellation and high-permission parent-child cancellation remain distinct', () => {
  const records = { release: { revision: 1, state: 'assembled', product: 'pinvou', component: 'app', targetKeys: ['a', 'b'] },
    a: { revision: 1, state: 'rejected', releaseKey: 'release' }, b: { revision: 1, state: 'in_review', releaseKey: 'release' }, operation: operation() };
  const command = { ...operationCommand, releaseKey: 'release', toState: 'closed' };
  assert.throws(() => planReleaseTransition(records, command, NOW + 1));
  assert.throws(() => planReleaseTransition(records, { ...command, toState: 'cancelled', actor: 'automatic' }, NOW + 1));
  records.b.state = 'approved';
  assert.equal(applyAtomically(records, planReleaseTransition(records, command, NOW + 1), NOW + 1).records.release.state, 'closed');
  records.approval = { revision: 1, projectionOwner: 'T05', state: 'approved', bodySha256: objectHash(releaseCancellationBody(records, 'release')),
    authorId: 'author', reviewerIds: ['reviewer-a', 'reviewer-b'], context: approvalContext('CancelRelease', records.release, 'release', 1) };
  const cancel = { ...command, toState: 'cancelled', actor: 'operator', approvalKey: 'approval', approvedBodySha256: HASH };
  const cancelled = applyAtomically(records, planReleaseTransition(records, cancel, NOW + 1), NOW + 1).records;
  assert.equal(cancelled.b.state, 'revoked'); assert.equal(cancelled.release.state, 'cancelled');
});
test('first activation stages two heads/complete reads; 15m equality rejects and 24h cleanup is fixed', () => {
  const fixture = publicationFixture(); const records = fixture.records;
  Object.assign(records.scope, { state: 'unactivated', selectionGeneration: 0, baselineDeploymentKey: null, rolloutKeys: [] });
  Object.assign(records.head, { published: false, revision: 0, bundle: null, rootHeadRevisionAtPublish: null });
  delete records.rollout; records.baseline.state = 'in_review'; records.inventory.requiredEntityKeys = records.inventory.requiredEntityKeys.filter((key) => key !== 'rollout' && !key.startsWith('candidate'));
  const bundle = proposedPublication(fixture, records, { now: NOW + 1 }).bundle;
  refreshPaths(records, { deploymentKey: 'baseline', anticipatedRevision: 2, now: NOW + 1 });
  const schedule = { ...operationCommand, scopeKey: 'scope', deploymentKey: 'baseline', expectedDeploymentRevision: 1,
    rootKey: 'root', headKey: 'head', denyKey: 'deny', inventoryKey: 'inventory', stagedSetKey: 'staged-set', bundle, pathKey: 'path' };
  let staged = applyAtomically(records, planScheduleFirst(records, schedule, NOW + 1), NOW + 1).records;
  staged.operation = operation(NOW + 2);
  const activate = { ...operationCommand, scopeKey: 'scope', deploymentKey: 'baseline', stagedSetKey: 'staged-set', pathKey: 'path',
    publication: { ...proposedPublication(fixture, staged, { now: NOW + 1 }), bundle } };
  const plan = planActivateFirst(staged, activate, NOW + 2); const changed = structuredClone(staged); changed.root.revision += 1;
  assert.throws(() => applyAtomically(changed, plan, NOW + 2));
  const active = applyAtomically(staged, plan, NOW + 2).records;
  assert.equal(active.scope.state, 'active'); assert.equal(active.scope.selectionGeneration, 1); assert.equal(active.scope.baselineDeploymentKey, 'baseline');
  assert.equal(stagedSetCleanupDue(active['staged-set'], NOW + 2 + DAY - 1), false);
  assert.equal(stagedSetCleanupDue(active['staged-set'], NOW + 2 + DAY), true);
  assert.throws(() => planActivateFirst(staged, activate, staged['staged-set'].expiresAt), { code: 'MODEL_FIRST_ACTIVATION_INVALID' });
});
