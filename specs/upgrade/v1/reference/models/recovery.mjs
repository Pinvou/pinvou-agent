import { requireCondition } from '../errors.mjs';
import { sameJson } from '../semantics.mjs';
import { atomicPlan, nextRecord, captureRecord } from './atomic.mjs';
import { commandPlan } from './operation.mjs';
import { readRecord, objectHash, assertFields, assertIdentity } from './inputs.mjs';
import { approvalReads, approvalContext, chainReads, assertCandidateAboveBaseline } from './entities.mjs';
import { assessStage } from './quality.mjs';
import { publicationParts } from './publication.mjs';
import { rolloutCommitment } from '../digests.mjs';
import { pathReads } from './paths.mjs';
import { currentReleaseHash } from './metadata-inputs.mjs';
import { assertUnchangedPaths } from './target-policy.mjs';

export function planRetireExecution(records, command, now) {
  const attempt = readRecord(records, command.attemptKey); const protection = readRecord(records, command.protectionKey);
  const proof = readRecord(records, command.proofKey);
  assertFields(attempt, { firstSentAt: 'integer', authorizationJti: 'id', transactionId: 'id', ownerEpoch: 'positive',
    oldIdentitySha256: 'hash', oldDataSha256: 'hash', state: ['consume_pending'], installationScopeId: 'id',
    purpose: ['install', 'preinstall', 'activate'] });
  requireCondition(['wait_exhausted', 'user_stopped', 'trusted_budget_unavailable'].includes(command.trigger)
    && (command.trigger !== 'wait_exhausted' || now >= attempt.firstSentAt + 300_000)
    && Number.isSafeInteger(now) && now >= attempt.firstSentAt
    && attempt.installationScopeId === protection.installationScopeId
    && proof.installationScopeId === attempt.installationScopeId && proof.executionPurpose === attempt.purpose
    && proof.transactionId === attempt.transactionId && proof.authorizationJti === attempt.authorizationJti
    && protection.ownerEpoch === attempt.ownerEpoch
    && ['preparing', 'executing'].includes(protection.state) && proof.stoppedOwnerEpoch === attempt.ownerEpoch
    && proof.boundaryAt === null && proof.activeMutationCount === 0 && proof.activeWriterCount === 0
    && proof.allExecutorsStopped === true && proof.oldIdentitySha256 === attempt.oldIdentitySha256
    && proof.oldDataSha256 === attempt.oldDataSha256, 'MODEL_RETIREMENT_UNPROVEN');
  const fence = { revision: 1, state: 'permanent', authorizationJti: attempt.authorizationJti,
    transactionId: attempt.transactionId, retiredOwnerEpoch: attempt.ownerEpoch, proofSha256: objectHash(proof), retiredAt: now };
  return atomicPlan(records, [command.attemptKey, command.protectionKey, command.proofKey], [
    { key: command.attemptKey, value: nextRecord(attempt, { state: 'local_execution_retired', fenceKey: command.fenceKey }) },
    { key: command.fenceKey, value: fence },
    { key: command.protectionKey, value: nextRecord(protection, { state: 'reconciliation_required',
      ownerEpoch: protection.ownerEpoch + 1, retiredAttemptKey: command.attemptKey, ownFreezeState: 'released' }) },
  ], [{ kind: 'local-execution-retired', transactionId: attempt.transactionId, committedAt: now }], now);
}
export function assertExecutionNotRetired(fence, { authorizationJti, transactionId, ownerEpoch }) {
  requireCondition(fence === null || fence.authorizationJti !== authorizationJti
    && fence.transactionId !== transactionId && ownerEpoch > fence.retiredOwnerEpoch, 'MODEL_EXECUTION_PERMANENTLY_RETIRED');
}
export function planReconcileRetired(records, command, now) {
  const task = readRecord(records, command.taskKey); const attempt = readRecord(records, command.attemptKey);
  const fence = readRecord(records, attempt.fenceKey); const proof = readRecord(records, command.proofKey);
  const protection = readRecord(records, command.protectionKey);
  const server = readRecord(records, command.serverDispositionKey);
  const occupancy = readRecord(records, server.occupancyKey);
  const history = readRecord(records, server.historyIndexKey);
  const reads = [command.taskKey, command.attemptKey, attempt.fenceKey, command.proofKey, command.protectionKey];
  requireCondition(server.projectionOwner === 'T22' && server.state === 'reviewed'
    && server.originalAuthorizationJti === attempt.authorizationJti && server.originalTransactionId === attempt.transactionId
    && server.originalConsumeRequestDigest === attempt.consumeRequestDigest
    && server.installationScopeId === attempt.installationScopeId && server.originalPurpose === attempt.purpose
    && server.historicalOutcome === task.historicalOutcome && server.taskId === task.taskId
    && occupancy.state === 'reconciliation_required' && occupancy.installationScopeId === attempt.installationScopeId
    && occupancy.originalAuthorizationJti === attempt.authorizationJti && occupancy.originalTransactionId === attempt.transactionId
    && history.installationScopeId === attempt.installationScopeId
    && (history.byAuthorizationJti[attempt.authorizationJti] ?? null) === server.authorizationKey
    && Array.isArray(server.readSet) && server.readSet.some((item) => item.key === server.occupancyKey)
    && server.readSet.some((item) => item.key === server.historyIndexKey), 'MODEL_RECONCILIATION_UNPROVEN');
  reads.push(command.serverDispositionKey, server.occupancyKey, server.historyIndexKey);
  for (const read of server.readSet) {
    requireCondition(sameJson(read, captureRecord(records, read.key)), 'MODEL_RECONCILIATION_UNPROVEN'); reads.push(read.key);
  }
  if (server.authorizationKey !== null) {
    const authorization = readRecord(records, server.authorizationKey); reads.push(server.authorizationKey);
    requireCondition(authorization.authorizationJti === attempt.authorizationJti
      && server.readSet.some((item) => item.key === server.authorizationKey)
      && ['consumed', 'cancelled', 'expired'].includes(authorization.state), 'MODEL_RECONCILIATION_UNPROVEN');
  } else requireCondition(server.historicalOutcome === 'unknown',
    'MODEL_RECONCILIATION_UNPROVEN');
  reads.push(...approvalReads(records, command.approvalKey, objectHash(task.binding), 2,
    approvalContext('ReconcileRetiredExecution', attempt, task.taskId, task.revision)));
  requireCondition(task.state === 'observing' && task.taskPurpose === 'reconcile' && now >= task.windowStart && now < task.windowEnd
    && task.binding.originalTransactionId === attempt.transactionId
    && task.binding.originalAuthorizationJti === attempt.authorizationJti
    && task.binding.originalConsumeRequestDigest === attempt.consumeRequestDigest
    && task.binding.originalPurpose === attempt.purpose
    && attempt.installationScopeId === protection.installationScopeId
    && task.binding.installationScopeId === protection.installationScopeId
    && fence.state === 'permanent' && proof.stoppedOwnerEpoch === fence.retiredOwnerEpoch
    && proof.allExecutorsStopped === true && proof.activeWriterCount === 0
    && proof.boundaryAt === null && proof.activeMutationCount === 0
    && proof.installationScopeId === attempt.installationScopeId && proof.executionPurpose === attempt.purpose
    && proof.transactionId === attempt.transactionId && proof.authorizationJti === attempt.authorizationJti
    && proof.oldIdentitySha256 === attempt.oldIdentitySha256 && proof.oldDataSha256 === attempt.oldDataSha256
    && protection.state === 'reconciliation_required' && protection.retiredAttemptKey === command.attemptKey,
  'MODEL_RECONCILIATION_UNPROVEN');
  return commandPlan(records, command, now, [...new Set(reads)], [
    { key: command.taskKey, value: nextRecord(task, { state: 'completed' }) },
    { key: command.dispositionKey, value: { revision: 1, taskId: task.taskId, state: 'independent-safety-disposition',
      historicalOutcome: task.historicalOutcome, proofSha256: objectHash(proof), committedAt: now } },
    { key: command.protectionKey, value: nextRecord(protection, { state: 'idle', retiredAttemptKey: null }) },
    { key: server.occupancyKey, value: nextRecord(occupancy, { state: 'released', dispositionKey: command.dispositionKey }) },
  ], [], { state: 'reconciled', dispositionKey: command.dispositionKey });
}

export function planReleaseQualityFreeze(records, command, now) {
  const rollout = readRecord(records, command.rolloutKey); const index = readRecord(records, command.freezeIndexKey);
  const review = readRecord(records, command.reviewKey); const stage = readRecord(records, rollout.stageFactKey);
  const plan = readRecord(records, rollout.planKey);
  const source = readRecord(records, review.sourceHeadKey);
  const scope = readRecord(records, rollout.scopeKey); const deployment = readRecord(records, rollout.deploymentKey);
  requireCondition(rollout.state === 'running' && rollout.qualityFrozen === true && review.state === 'observing'
    && review.rolloutKey === command.rolloutKey && review.freezeIndexRevision === index.revision
    && review.planSha256 === rollout.planSha256 && review.stageFactRevision === stage.revision
    && plan.revision === rollout.planRevision && objectHash(plan.body) === rollout.planSha256
    && review.currentStage !== null && typeof review.currentStage === 'object'
    && now >= review.windowStart && now < review.windowEnd
    && review.sourceHeadKey === scope.contributionHeadKey && scope.runningRolloutKey === command.rolloutKey
    && stage.stageId === plan.body.stages[rollout.stageIndex].stageId
    && index.reasonKeys.length > 0 && sameJson([...review.reasonKeys].sort(), [...index.reasonKeys].sort())
    && sameJson(review.currentStage.plan, plan.body.stages[rollout.stageIndex]), 'MODEL_REVIEW_STALE');
  const reads = [command.rolloutKey, command.freezeIndexKey, command.reviewKey, rollout.stageFactKey, rollout.planKey,
    review.sourceHeadKey, rollout.scopeKey, rollout.deploymentKey];
  reads.push(...chainReads(records, deployment.chain, now, { expectedScope: scope }), assertCandidateAboveBaseline(records, scope, deployment));
  reads.push(...pathReads(records, command.pathKey, rollout.deploymentKey, rollout.scopeKey, now,
    { targetBundle: command.publication.bundle }));
  reads.push(...approvalReads(records, command.approvalKey, objectHash({ command: 'ReleaseQualityFreeze',
    reviewKey: command.reviewKey, review, rolloutKey: command.rolloutKey, rolloutRevision: rollout.revision }), 2,
  approvalContext('ReleaseQualityFreeze', scope, command.reviewKey, review.revision)));
  const writes = [];
  requireCondition(review.groups.length === index.reasonKeys.length
    && new Set(review.groups.map((group) => group.reasonKey)).size === review.groups.length, 'MODEL_QUALITY_INCOMPLETE');
  for (const key of index.reasonKeys) {
    const reason = readRecord(records, key); reads.push(key);
    const group = review.groups.find((candidate) => candidate.reasonKey === key);
    requireCondition(reason.state === 'frozen' && group !== undefined && sameJson(group.originalPlan, reason.originalPlan)
      && group.observation.startedAt === review.windowStart
      && assessStage(group.originalPlan, group.observation, now, reason.originalGroupIdentities).passed, 'QUALITY_INCOMPLETE');
    writes.push({ key, value: nextRecord(reason, { state: 'released', releasedAt: now }) });
  }
  requireCondition(review.currentStage.observation.startedAt === review.windowStart
    && assessStage(review.currentStage.plan, review.currentStage.observation, now, source.windowGroups[review.windowId]).passed
    && review.sourceWatermark === source.watermark && review.processedWatermark === source.watermark
    && review.targetIdentitySha256 === rollout.targetIdentitySha256,
  'QUALITY_INCOMPLETE');
  assertIdentity(command.newWindowId);
  requireCondition(command.newWindowId !== review.windowId && command.newWindowId !== stage.windowId
    && !(rollout.observationWindowIds ?? []).includes(command.newWindowId)
    && !Object.hasOwn(source.windowGroups, command.newWindowId), 'MODEL_OBSERVATION_INVALID');
  const publication = publicationParts(records, command.publication, now); const opening = command.opening;
  requireCondition(command.publication.mode === 'selection' && sameJson(command.publication.affectedScopeKeys, [rollout.scopeKey])
    && opening !== null && opening.targetKey === scope.targetKey && opening.deploymentId === deployment.deploymentId
    && opening.deploymentRevision === deployment.revision && opening.rolloutId === rollout.rolloutId
    && opening.rolloutRevision === rollout.revision + 1
    && opening.releaseEnvelopeSha256 === currentReleaseHash(records, deployment.releaseKey)
    && (scope.currentCandidateOpening == null || opening.leafSalt !== scope.currentCandidateOpening.leafSalt), 'MODEL_OPENING_INVALID');
  const target = command.publication.bundle.members.find((member) => member.signed.role === 'target'
    && member.signed.scope.channel === scope.channel && member.signed.scope.targetKey === scope.targetKey);
  const previousTarget = records[command.publication.headKey].bundle.members.find((member) => member.signed.role === 'target'
    && member.signed.scope.channel === scope.channel && member.signed.scope.targetKey === scope.targetKey);
  assertUnchangedPaths(previousTarget.signed, target.signed);
  requireCondition(target !== undefined && target.signed.rolloutSetCommitment === rolloutCommitment(opening)
    && target.signed.baselineRelease.envelopeSha256 === currentReleaseHash(records, records[scope.baselineDeploymentKey].releaseKey),
  'MODEL_OPENING_INVALID');
  const scopeWrite = publication.writes.find((write) => write.key === rollout.scopeKey);
  requireCondition(scopeWrite !== undefined, 'MODEL_PUBLICATION_REQUIRED');
  scopeWrite.value.currentCandidateOpening = structuredClone(opening);
  reads.push(...publication.reads); writes.push(...publication.writes);
  writes.push({ key: command.rolloutKey, value: nextRecord(rollout, { qualityFrozen: false,
    observationWindowIds: [...(rollout.observationWindowIds ?? []), command.newWindowId],
    postRecoveryReasonKeys: [...new Set([...(rollout.postRecoveryReasonKeys ?? []), ...index.reasonKeys])] }) },
    { key: command.freezeIndexKey, value: nextRecord(index, { reasonKeys: [], releasedAt: now }) },
    { key: command.reviewKey, value: nextRecord(review, { state: 'completed' }) },
    { key: rollout.stageFactKey, value: nextRecord(stage, { kind: 'quality-unfrozen', windowId: command.newWindowId,
      effectiveAt: now, sourceWatermark: source.watermark }) });
  return commandPlan(records, command, now, [...new Set(reads)], writes,
    [...publication.facts, { kind: 'quality-unfrozen', rolloutKey: command.rolloutKey, committedAt: now }], { percentage: rollout.percentage });
}
