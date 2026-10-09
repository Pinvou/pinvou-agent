import test from 'node:test';
import assert from 'node:assert/strict';
import { applyAtomically, assertWorkerLease, operationReplay } from '../models/atomic.mjs';
import { planRenewOwner, planAcquireOwner } from '../models/operation.mjs';
import { planDownloadHandoff, downloadObservation, workflowContext, hopIdentitySha256 } from '../models/workflow.mjs';
import { metricAssessment, assessStage, projectEvidence } from '../models/quality.mjs';
import { objectHash } from '../models/inputs.mjs';
import { modelSession, addQualification, operation, operationCommand } from './model-fixtures.mjs';
import { NOW, HASH, createFixtureSet } from './fixtures.mjs';
import { DAY } from '../semantics.mjs';

test('download handoff after 24h preserves content and original attribution without a total deadline', () => {
  const now = NOW + 40 * DAY; const set = createFixtureSet({ clock: now });
  const decision = set.claims.decision; decision.telemetrySessionId = 'session-2'; decision.update.originalStage.firstQualifiedAt = NOW;
  const immutableContext = workflowContext(decision); const immutableContextSha256 = objectHash(immutableContext);
  const oldSession = { ...modelSession('expired'), lastStateBeforeTerminal: 'download_started', immutableContextSha256,
    immutableContext, hopSha256: hopIdentitySha256(decision) };
  const records = { oldSession, workflow: { revision: 1, state: 'ongoing', workflowId: 'workflow-1', writableSessionId: 'session-1',
    immutableContext, firstStartedAt: NOW, lastObservedAt: now, lossIntervalMs: 120_000, confirmedHopSha256: oldSession.hopSha256 },
    operation: operation(now), preparation: { revision: 1, state: 'idle', installationScopeId: 'scope-1', ownerEpoch: 2,
      dataScopeSha256: HASH, freezeEpoch: null, backup: null, transactionId: null },
    root: { revision: 1, published: true, body: set.metadata.root }, index: { revision: 1, installationScopeId: 'scope-1', memberKeys: ['oldSession'] } };
  addQualification(records, { immutableContextSha256 }, { owner: 'T17', now, claims: decision }); records.qualification.contentIdentity = immutableContext.contentIdentity;
  const command = { ...operationCommand, workflowKey: 'workflow', oldSessionKey: 'oldSession', newSessionKey: 'newSession', scopeIndexKey: 'index',
    qualificationKey: 'qualification', expectedRevision: 1, preparationKey: 'preparation', timeoutKey: 'new-session-timeout',
    diagnosticCursorKey: 'new-diagnostic', rootKey: 'root', decisionBytes: set.sign(decision),
    expectedContext: { role: 'decision', product: decision.product, component: decision.component, scope: decision.scope } };
  const plan = planDownloadHandoff(records, command, now); const result = applyAtomically(records, plan, now).records;
  assert.equal(result.oldSession.futureActions, 'fenced'); assert.equal(result.workflow.writableSessionId, 'session-2');
  assert.deepEqual(result.workflow.immutableContext, immutableContext); assert.equal(result.workflow.firstStartedAt, NOW);
  assert.equal(result.newSession.state, 'decision_received'); assert.equal(result.newSession.sequence, 0);
  assert.equal(result.newSession.preparationEpoch, 2); assert.equal(result.newSession.freezeEpoch, null);
  assert.equal(result.newSession.authorizationKey, null); assert.equal(result.newSession.activeValidateKey, null);
  assert.equal(result.newSession.installId, decision.installId); assert.equal(result.newSession.decisionId, decision.decisionId);
  assert.equal(result.newSession.decisionRevision, decision.decisionRevision); assert.equal(result.newSession.groupIdentity, oldSession.groupIdentity);
  assert.throws(() => planDownloadHandoff(records, { ...command, newSession: { ...oldSession, sequence: 8 } }, now));
  assert.equal(downloadObservation(result.workflow, now), 'incomplete');
  assert.equal(downloadObservation(result.workflow, now + 119_999), 'incomplete');
  assert.equal(downloadObservation(result.workflow, now + 120_000), 'outcome_unknown');
  const changed = structuredClone(records); changed['qualified-endpoint'].revision += 1;
  assert.throws(() => planDownloadHandoff(changed, command, now), { code: 'MODEL_QUALIFICATION_LOST' });
  assert.throws(() => applyAtomically(result, plan, now), { code: 'MODEL_CAS_CONFLICT' });
});
test('exact quality ratios: equality fails, zero samples fail, incomplete alone never freezes', () => {
  const policy = { minimumSamples: 10, threshold: { numerator: 1, denominator: 10 } };
  assert.equal(metricAssessment({ succeeded: 9, failed: 1, incomplete: 0, unknown: 0 }, policy).realBelow, false);
  assert.equal(metricAssessment({ succeeded: 10, failed: 0, incomplete: 0, unknown: 0 }, policy).upperBelow, true);
  assert.equal(metricAssessment({ succeeded: 10, failed: 0, incomplete: 100, unknown: 0 }, policy).unknownFailureThresholdReached, false);
  assert.equal(metricAssessment({ succeeded: 10, failed: 0, incomplete: 100, unknown: 1 }, policy).unknownFailureThresholdReached, true);
  const zero = metricAssessment({ succeeded: 0, failed: 0, incomplete: 0, unknown: 0 }, policy);
  assert.equal(zero.sufficient, false); assert.equal(zero.realBelow, false);
});
test('quality requires every approved group/metric and complete observation time', () => {
  const policy = { minimumSamples: 10, threshold: { numerator: 1, denominator: 10 } };
  const plan = { minimumObservationMs: 1000, requiredMetrics: ['download'], groupIdentities: ['group-a', 'group-b'], metrics: { download: policy } };
  const metric = { succeeded: 10, failed: 0, incomplete: 0, unknown: 0 };
  const observation = { startedAt: NOW, groups: ['group-a', 'group-b'].map((groupIdentity) => ({ groupIdentity, metrics: { download: metric } })) };
  assert.equal(assessStage(plan, observation, NOW + 999, plan.groupIdentities).passed, false);
  assert.equal(assessStage(plan, observation, NOW + 1000, plan.groupIdentities).passed, true);
  assert.throws(() => assessStage({ ...plan, requiredMetrics: [] }, observation, NOW + 1000, plan.groupIdentities));
  assert.throws(() => assessStage(plan, { ...observation, groups: observation.groups.slice(0, 1) }, NOW + 1000, plan.groupIdentities));
  const missing = structuredClone(observation); delete missing.groups[1].metrics.download;
  assert.throws(() => assessStage(plan, missing, NOW + 1000, plan.groupIdentities));
});
test('quality evidence preserves failure, rejects wrong binding and cannot regress success with older progress', () => {
  const sample = { workflowId: 'workflow-1', groupIdentity: HASH, targetIdentitySha256: HASH, installationScopeId: 'scope-1',
    metric: 'installation', windowId: 'window-1', lastContributionWatermark: 1, events: [], outcome: 'failed' };
  const event = { ...sample, eventId: 'event-1', sha256: HASH, contributionWatermark: 2, kind: 'state-result', outcome: 'succeeded' };
  assert.equal(projectEvidence(sample, event).outcome, 'failed');
  const success = { ...sample, outcome: 'succeeded' };
  assert.equal(projectEvidence(success, { ...event, outcome: 'incomplete' }).outcome, 'succeeded');
  assert.equal(projectEvidence({ ...sample, outcome: 'outcome_unknown' }, { ...event, kind: 'higher-version-repair' }).outcome, 'outcome_unknown');
  assert.throws(() => projectEvidence(sample, { ...event, windowId: 'other' }));
  const projected = projectEvidence(success, event);
  assert.equal(projectEvidence({ ...success, lastContributionWatermark: 3 }, { ...event, outcome: 'failed' }).outcome, 'failed');
  assert.throws(() => projectEvidence(projected, { ...event, sha256: 'b'.repeat(64) }), { code: 'IDEMPOTENCY_CONFLICT' });
});
test('30s leases, 10s renewal, 2m ownership and 24h replay do not extend original rights', () => {
  const records = { operation: operation() };
  assertWorkerLease(records.operation, 1, NOW + 29_999);
  assert.throws(() => assertWorkerLease(records.operation, 1, NOW + 30_000));
  assert.throws(() => planRenewOwner(records, { operationKey: 'operation', ownerEpoch: 1 }, NOW + 9999));
  const renewed = applyAtomically(records, planRenewOwner(records, { operationKey: 'operation', ownerEpoch: 1 }, NOW + 10_000), NOW + 10_000).records;
  assert.equal(renewed.operation.leaseExpiresAt, NOW + 40_000);
  const takeover = applyAtomically(records, planAcquireOwner(records, { operationKey: 'operation', expectedRevision: 1 }, NOW + 30_000), NOW + 30_000).records;
  assert.equal(takeover.operation.ownerEpoch, 2);
  assert.throws(() => assertWorkerLease(takeover.operation, 1, NOW + 30_000));
  assert.throws(() => assertWorkerLease({ ...records.operation, leaseExpiresAt: NOW + 200_000 }, 1, NOW + 120_000));
  const committed = { requestDigest: HASH, state: 'committed', committedAt: NOW, resultId: 'result-1', result: { credentialExpiresAt: NOW + 300_000 } };
  assert.equal(operationReplay(committed, HASH, NOW + DAY - 1).result.credentialExpiresAt, NOW + 300_000);
  assert.deepEqual(operationReplay(committed, HASH, NOW + DAY), { kind: 'minimal', resultId: 'result-1' });
  assert.throws(() => operationReplay(committed, 'b'.repeat(64), NOW), { code: 'IDEMPOTENCY_CONFLICT' });
});
