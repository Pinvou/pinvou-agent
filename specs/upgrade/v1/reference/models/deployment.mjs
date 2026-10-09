import { requireCondition } from '../errors.mjs';
import { nextRecord } from './atomic.mjs';
import { commandPlan } from './operation.mjs';
import { readRecord, assertIdentity } from './inputs.mjs';
import { chainReads } from './entities.mjs';
import { pathReads } from './paths.mjs';
import { currentReleaseHash } from './metadata-inputs.mjs';
import { publicationParts } from './publication.mjs';
import { rolloutCommitment } from '../digests.mjs';
import { resumptionReads } from './resumption.mjs';
import { sameJson } from '../semantics.mjs';
import { assertUnchangedPaths } from './target-policy.mjs';

/** Baseline parent lifecycle has no candidate-above-baseline condition. A
 * completed child is read for CAS but cannot be restarted or rewritten here.
 */
export function planBaselineCommand(records, command, now) {
  const scope = readRecord(records, command.scopeKey); const deployment = readRecord(records, command.deploymentKey);
  requireCondition(scope.state === 'active' && scope.baselineDeploymentKey === command.deploymentKey
    && deployment.revision === command.expectedRevision && deployment.channel === scope.channel
    && deployment.product === scope.product && deployment.component === scope.component && deployment.targetKey === scope.targetKey,
  'MODEL_BASELINE_INVALID');
  const reads = [command.scopeKey, command.deploymentKey];
  if (deployment.rolloutKey !== null) {
    const child = readRecord(records, deployment.rolloutKey); reads.push(deployment.rolloutKey);
    requireCondition(child.state === 'completed' && child.deploymentKey === command.deploymentKey, 'MODEL_BASELINE_INVALID');
  }
  const resuming = command.action === 'resume';
  requireCondition(resuming ? deployment.state === 'paused' : command.action === 'pause' && deployment.state === 'active', 'MODEL_EDGE_INVALID');
  if (resuming) {
    reads.push(...resumptionReads(records, command, command.deploymentKey, scope));
    reads.push(...chainReads(records, deployment.chain, now, { expectedScope: scope, resuming: true }),
      ...pathReads(records, command.pathKey, command.deploymentKey, command.scopeKey, now,
        { resuming: true, targetBundle: command.publication.bundle }));
  } else assertIdentity(command.reason);
  const publication = publicationParts(records, command.publication, now);
  requireCondition(command.publication.mode === 'selection' && sameJson(command.publication.affectedScopeKeys, [command.scopeKey]),
    'MODEL_PUBLICATION_REQUIRED');
  const target = command.publication.bundle.members.find((member) => member.signed.role === 'target'
    && member.signed.scope.channel === scope.channel && member.signed.scope.targetKey === scope.targetKey);
  const previousTarget = records[command.publication.headKey].bundle.members.find((member) => member.signed.role === 'target'
    && member.signed.scope.channel === scope.channel && member.signed.scope.targetKey === scope.targetKey);
  assertUnchangedPaths(previousTarget.signed, target.signed);
  const opening = command.opening;
  if (!scope.hasCandidate) requireCondition(opening === null, 'MODEL_OPENING_INVALID');
  else {
    const candidate = readRecord(records, scope.runningRolloutKey); const parent = readRecord(records, candidate.deploymentKey);
    reads.push(scope.runningRolloutKey, candidate.deploymentKey, parent.releaseKey);
    requireCondition(opening !== null && opening.targetKey === scope.targetKey && opening.deploymentId === parent.deploymentId
      && opening.deploymentRevision === parent.revision && opening.rolloutId === candidate.rolloutId
      && opening.rolloutRevision === candidate.revision && opening.releaseEnvelopeSha256 === currentReleaseHash(records, parent.releaseKey)
      && opening.leafSalt !== scope.currentCandidateOpening.leafSalt, 'MODEL_OPENING_INVALID');
  }
  requireCondition(target !== undefined && target.signed.rolloutSetCommitment === rolloutCommitment(opening)
    && target.signed.baselineRelease.envelopeSha256 === currentReleaseHash(records, deployment.releaseKey), 'MODEL_BASELINE_INVALID');
  const scopeWrite = publication.writes.find((write) => write.key === command.scopeKey);
  requireCondition(scopeWrite !== undefined, 'MODEL_PUBLICATION_REQUIRED');
  scopeWrite.value.currentCandidateOpening = structuredClone(opening);
  return commandPlan(records, command, now, [...new Set([...reads, ...publication.reads])], [...publication.writes,
    { key: command.deploymentKey, value: nextRecord(deployment, { state: resuming ? 'active' : 'paused',
      ...(resuming ? {} : { pauseReason: { reason: command.reason, pausedAt: now } }) }) }],
  [...publication.facts, { kind: `baseline-${command.action}`, committedAt: now }], { state: resuming ? 'active' : 'paused' });
}
