import { requireCondition } from '../errors.mjs';
import { sameJson } from '../semantics.mjs';
import { nextRecord } from './atomic.mjs';
import { commandPlan } from './operation.mjs';
import { readRecord, objectHash, assertIdentity } from './inputs.mjs';
import { chainReads, assertCandidateAboveBaseline, approvalReads, approvalContext } from './entities.mjs';
import { publicationParts } from './publication.mjs';
import { assessStage } from './quality.mjs';
import { rolloutCommitment } from '../digests.mjs';
import { assertApiShape } from '../api-registry.mjs';
import { assertStagePlan } from './plans.mjs';
export { assertStagePlan } from './plans.mjs';
import { pathReads } from './paths.mjs';
import { currentReleaseHash } from './metadata-inputs.mjs';
import { resumptionReads } from './resumption.mjs';
import { bridgeUnitParts } from './bridge-unit.mjs';
import { assertUnchangedPaths } from './target-policy.mjs';

function qualityReads(records, command, rollout, plan, stageFact, now) {
  const quality = readRecord(records, command.qualityKey); const reasons = readRecord(records, command.freezeIndexKey);
  assertApiShape('quality-projection', quality); assertApiShape('stage-fact', stageFact);
  const source = readRecord(records, quality.sourceHeadKey);
  // Existing workflows retain original window/group attribution even when a
  // Registry update changes the independently checked future selectable paths.
  requireCondition(quality.rolloutKey === command.rolloutKey && quality.planSha256 === rollout.planSha256
    && quality.stageId === plan.stages[rollout.stageIndex].stageId && quality.windowId === stageFact.windowId
    && quality.stageFactRevision === stageFact.revision && quality.observation.startedAt === stageFact.effectiveAt
    && quality.processedWatermark === source.watermark && quality.sourceWatermark === source.watermark
    && quality.freezeIndexRevision === reasons.revision && Array.isArray(reasons.reasonKeys), 'MODEL_QUALITY_STALE');
  requireCondition(quality.sourceHeadKey === records[rollout.scopeKey].contributionHeadKey
    && source.watermark >= stageFact.sourceWatermark, 'MODEL_QUALITY_STALE');
  const reads = [command.qualityKey, command.freezeIndexKey, quality.sourceHeadKey];
  for (const key of reasons.reasonKeys) {
    reads.push(key); requireCondition(readRecord(records, key).state === 'released', 'MODEL_QUALITY_FROZEN');
  }
  requireCondition(assessStage(plan.stages[rollout.stageIndex], quality.observation, now,
    source.windowGroups[stageFact.windowId]).passed, 'QUALITY_INCOMPLETE');
  const requiredReasons = rollout.postRecoveryReasonKeys ?? [];
  const history = quality.historicalObservations ?? [];
  requireCondition(sameJson([...requiredReasons].sort(), history.map((item) => item.reasonKey).sort()), 'MODEL_QUALITY_INCOMPLETE');
  for (const item of history) {
    const reason = readRecord(records, item.reasonKey); reads.push(item.reasonKey);
    requireCondition(reason.state === 'released' && item.observation.startedAt === stageFact.effectiveAt
      && assessStage(reason.originalPlan, item.observation, now, reason.originalGroupIdentities).passed, 'QUALITY_INCOMPLETE');
  }
  return reads;
}

/** T47-owned stage facts are emitted in the same atomic publication commit.
 * T21 consumes their IDs/revisions/watermarks; no precomputed 'quality passed'
 * flag, manual percentage or old-stage success can authorize a transition.
 */
export function planRolloutCommand(records, command, now) {
  const rollout = readRecord(records, command.rolloutKey); const deployment = readRecord(records, rollout.deploymentKey);
  const scope = readRecord(records, rollout.scopeKey); const planRecord = readRecord(records, rollout.planKey);
  const plan = planRecord.body;
  assertStagePlan(plan);
  requireCondition(plan.upgradeType === deployment.upgradeType
    && rollout.revision === command.expectedRevision && planRecord.revision === rollout.planRevision
    && objectHash(plan) === rollout.planSha256 && deployment.rolloutKey === command.rolloutKey,
  'MODEL_PLAN_CHANGED');
  const reads = [command.rolloutKey, rollout.deploymentKey, rollout.scopeKey, rollout.planKey];
  const sourceHead = readRecord(records, scope.contributionHeadKey); reads.push(scope.contributionHeadKey);
  const indexedRollouts = Object.entries(records).filter(([, record]) => record.recordKind === 'rollout'
    && record.scopeKey === rollout.scopeKey).map(([key]) => key).sort();
  requireCondition(Array.isArray(scope.rolloutKeys) && sameJson(indexedRollouts, [...scope.rolloutKeys].sort()), 'MODEL_ROLLOUT_INDEX_INCOMPLETE');
  for (const key of scope.rolloutKeys) {
    const other = readRecord(records, key); reads.push(key);
    requireCondition(other.deploymentKey !== rollout.deploymentKey || key === command.rolloutKey
      || ['completed', 'aborted'].includes(other.state), 'MODEL_ROLLOUT_DUPLICATE');
    requireCondition(other.state !== 'running' || scope.runningRolloutKey === key, 'MODEL_ROLLOUT_DUPLICATE');
  }
  reads.push(...approvalReads(records, planRecord.approvalKey, rollout.planSha256, deployment.channel === 'stable' ? 2 : 1,
    approvalContext('FreezeRolloutPlan', scope, rollout.planKey, planRecord.revision)));
  const publication = publicationParts(records, command.publication, now);
  requireCondition(command.publication.mode === 'selection' && sameJson(command.publication.affectedScopeKeys, [rollout.scopeKey]),
    'MODEL_PUBLICATION_REQUIRED');
  if (command.action !== 'complete') {
    const previous = records[command.publication.headKey].bundle.members.find((member) => member.signed.role === 'target'
      && member.signed.scope.channel === scope.channel && member.signed.scope.targetKey === scope.targetKey);
    const next = command.publication.bundle.members.find((member) => member.signed.role === 'target'
      && member.signed.scope.channel === scope.channel && member.signed.scope.targetKey === scope.targetKey);
    assertUnchangedPaths(previous.signed, next.signed);
  }
  const prospectiveMetadata = (command.publication.privateReleaseUpdates?.length ?? 0) > 0 && command.action !== 'complete'
    ? Object.fromEntries(publication.writes.filter((write) => [command.publication.headKey, rollout.scopeKey].includes(write.key))
      .map((write) => [write.key, structuredClone(write.value)])) : null;
  const qualificationView = prospectiveMetadata === null ? records : { ...records, ...prospectiveMetadata };
  const writes = []; const action = command.action;
  const changes = {}; const deploymentChanges = {};
  if (['start', 'advance', 'resume'].includes(action)) {
    assertIdentity(command.windowId);
    requireCondition(!(rollout.observationWindowIds ?? []).includes(command.windowId)
      && !Object.hasOwn(sourceHead.windowGroups, command.windowId), 'MODEL_OBSERVATION_INVALID');
    changes.observationWindowIds = [...(rollout.observationWindowIds ?? []), command.windowId];
  }
  let fact = null;
  const factRecord = rollout.stageFactKey === null ? null : readRecord(records, rollout.stageFactKey);
  if (factRecord !== null) reads.push(rollout.stageFactKey);
  if (['start', 'advance', 'complete', 'resume'].includes(action)) {
    reads.push(...pathReads(records, command.pathKey, rollout.deploymentKey, rollout.scopeKey, now,
      { phase: action === 'complete' ? 'install' : 'display', resuming: action === 'resume',
        targetBundle: action === 'complete' ? null : command.publication.bundle, viewOverrides: prospectiveMetadata }));
    reads.push(...chainReads(qualificationView, deployment.chain, now, { phase: action === 'complete' ? 'install' : 'display',
      resuming: action === 'resume', expectedScope: scope }));
    // Resume sees the proposed parent state, while still comparing the actual
    // paused parent's revision in the final CAS.
    reads.push(assertCandidateAboveBaseline(records, scope, deployment));
  }
  if (action === 'start') {
    requireCondition(rollout.state === 'draft' && deployment.state === 'active' && rollout.stageIndex === null
      && rollout.stageFactKey === null && scope.runningRolloutKey === null && command.actor === 'system', 'MODEL_EDGE_INVALID');
    Object.assign(changes, { state: 'running', stageIndex: 0, percentage: plan.stages[0].percentage, stageFactKey: command.newStageFactKey });
    fact = { revision: 1, rolloutKey: command.rolloutKey, stageId: plan.stages[0].stageId, windowId: command.windowId,
      kind: 'started', effectiveAt: now, sourceWatermark: sourceHead.watermark, planRevision: rollout.planRevision, planSha256: rollout.planSha256 };
  } else if (action === 'advance' || action === 'complete') {
    requireCondition(rollout.state === 'running' && deployment.state === 'active' && scope.runningRolloutKey === command.rolloutKey
      && command.actor === 'system' && factRecord !== null && rollout.qualityFrozen === false, 'MODEL_EDGE_INVALID');
    requireCondition(rollout.percentage === plan.stages[rollout.stageIndex].percentage, 'MODEL_PLAN_CHANGED');
    reads.push(...qualityReads(records, command, rollout, plan, factRecord, now));
    if (action === 'advance') {
      requireCondition(rollout.stageIndex + 1 < plan.stages.length, 'MODEL_EDGE_INVALID');
      Object.assign(changes, { stageIndex: rollout.stageIndex + 1, percentage: plan.stages[rollout.stageIndex + 1].percentage,
        postRecoveryReasonKeys: [] });
      fact = nextRecord(factRecord, { stageId: plan.stages[changes.stageIndex].stageId, windowId: command.windowId,
        kind: 'advanced', effectiveAt: now, sourceWatermark: sourceHead.watermark });
    } else {
      requireCondition(rollout.stageIndex === plan.stages.length - 1 && rollout.percentage === 100, 'MODEL_FINAL_STAGE_REQUIRED');
      const oldKey = scope.baselineDeploymentKey; const old = readRecord(records, oldKey);
      writes.push({ key: oldKey, value: nextRecord(old, { state: 'superseded' }) });
      Object.assign(changes, { state: 'completed' });
      fact = nextRecord(factRecord, { kind: 'completed', effectiveAt: now });
    }
  } else if (action === 'pause') {
    requireCondition(rollout.state === 'running' && deployment.state === 'active' && factRecord !== null, 'MODEL_EDGE_INVALID');
    assertIdentity(command.reason);
    Object.assign(changes, { state: 'paused' }); Object.assign(deploymentChanges,
      { state: 'paused', pauseReason: { reason: command.reason, pausedAt: now } });
    fact = nextRecord(factRecord, { kind: 'paused', effectiveAt: now });
  } else if (action === 'resume') {
    requireCondition(rollout.state === 'paused' && deployment.state === 'paused' && factRecord !== null, 'MODEL_EDGE_INVALID');
    reads.push(...resumptionReads(records, command, rollout.deploymentKey, scope));
    Object.assign(changes, { state: 'running' }); Object.assign(deploymentChanges, { state: 'active' });
    // Quality freezing remains latched; eligibility restoration needs no new
    // samples during suspension. T21 observes a new complete window afterward.
    fact = nextRecord(factRecord, { kind: 'resumed', windowId: command.windowId, effectiveAt: now, sourceWatermark: sourceHead.watermark });
  } else if (action === 'abort') {
    requireCondition(['draft', 'running', 'paused'].includes(rollout.state)
      && ['operator_abort', 'parent_withdrawn', 'parent_superseded'].includes(command.abortCause), 'MODEL_ABORT_INVALID');
    requireCondition(command.abortCause === 'operator_abort' ? command.actor === 'operator'
      : command.actor === 'parent-command', 'MODEL_ABORT_INVALID');
    const parentState = command.abortCause === 'parent_superseded' ? 'superseded' : 'withdrawn';
    requireCondition(scope.baselineDeploymentKey !== rollout.deploymentKey, 'MODEL_BASELINE_SUPERSEDE_FORBIDDEN');
    Object.assign(changes, { state: 'aborted', abortCause: command.abortCause }); Object.assign(deploymentChanges, { state: parentState });
    if (factRecord !== null) fact = nextRecord(factRecord, { kind: 'aborted', effectiveAt: now });
  } else requireCondition(false, 'MODEL_COMMAND_UNKNOWN');
  reads.push(...publication.reads); writes.push(...publication.writes);
  const opening = command.opening;
  if (['complete', 'abort'].includes(action)) requireCondition(opening === null, 'MODEL_OPENING_INVALID');
  else requireCondition(opening !== null && opening.targetKey === scope.targetKey
    && opening.deploymentId === deployment.deploymentId && opening.deploymentRevision === deployment.revision + (Object.keys(deploymentChanges).length > 0 ? 1 : 0)
    && opening.rolloutId === rollout.rolloutId && opening.rolloutRevision === rollout.revision + 1
    && opening.releaseEnvelopeSha256 === currentReleaseHash({ ...records,
      [command.publication.headKey]: publication.writes.find((write) => write.key === command.publication.headKey).value }, deployment.releaseKey),
    'MODEL_OPENING_INVALID');
  const proposedTarget = command.publication.bundle.members.find((member) => member.signed.role === 'target'
    && member.signed.scope.channel === scope.channel && member.signed.scope.targetKey === scope.targetKey);
  requireCondition(proposedTarget.signed.rolloutSetCommitment === rolloutCommitment(opening), 'MODEL_OPENING_INVALID');
  requireCondition(proposedTarget.signed.baselineRelease.envelopeSha256 === currentReleaseHash({ ...records,
    [command.publication.headKey]: publication.writes.find((write) => write.key === command.publication.headKey).value },
    action === 'complete' ? deployment.releaseKey : records[scope.baselineDeploymentKey].releaseKey),
  'MODEL_BASELINE_INVALID');
  if (opening !== null && scope.currentCandidateOpening != null) requireCondition(
    opening.leafSalt !== scope.currentCandidateOpening.leafSalt, 'MODEL_CANDIDATE_SALT_REUSED');
  const scopeWrite = writes.find((write) => write.key === rollout.scopeKey);
  requireCondition(scopeWrite !== undefined, 'MODEL_PUBLICATION_REQUIRED');
  if (action === 'start') Object.assign(scopeWrite.value, { runningRolloutKey: command.rolloutKey, hasCandidate: true });
  if (action === 'complete') Object.assign(scopeWrite.value, { baselineDeploymentKey: rollout.deploymentKey, runningRolloutKey: null, hasCandidate: false });
  if (action === 'abort') Object.assign(scopeWrite.value, { runningRolloutKey: null, hasCandidate: false });
  Object.assign(scopeWrite.value, { currentCandidateOpening: structuredClone(opening) });
  if (Object.keys(deploymentChanges).length > 0) writes.push({ key: rollout.deploymentKey, value: nextRecord(deployment, deploymentChanges) });
  writes.push({ key: command.rolloutKey, value: nextRecord(rollout, changes) });
  if (fact !== null) writes.push({ key: rollout.stageFactKey ?? command.newStageFactKey, value: fact });
  if (action === 'complete') {
    const bridgeWrites = [];
    if (command.bridgeUnitKey !== undefined) {
      const bridge = bridgeUnitParts(records, command.bridgeUnitKey, scope, scope.baselineDeploymentKey, now);
      reads.push(...bridge.reads); writes.push(...bridge.writes); bridgeWrites.push(...bridge.writes);
    }
    const viewOverrides = Object.fromEntries(writes.filter((write) => [rollout.scopeKey, scope.baselineDeploymentKey,
      command.publication.headKey, ...bridgeWrites.map((entry) => entry.key)].includes(write.key)).map((write) => [write.key, write.value]));
    reads.push(...pathReads(records, command.futurePathKey, rollout.deploymentKey, rollout.scopeKey, now,
      { phase: 'install', targetBundle: command.publication.bundle, viewOverrides }));
  }
  return commandPlan(records, command, now, [...new Set(reads)], writes,
    [...publication.facts, { kind: `rollout-${action}`, committedAt: now, rolloutKey: command.rolloutKey,
      stageId: fact?.stageId ?? null, windowId: fact?.windowId ?? null }], { state: changes.state ?? rollout.state });
}
