import { requireCondition } from '../errors.mjs';
import { sameJson } from '../semantics.mjs';
import { readRecord, objectHash } from './inputs.mjs';
import { publicationParts, targetSelection } from './publication.mjs';
import { currentReleaseHash } from './metadata-inputs.mjs';
import { pathReads } from './paths.mjs';
import { commandPlan } from './operation.mjs';

/** Renew an existing hidden selection without changing its Deployment/Rollout
 * business revision, percentage or quality window. The changed opening requires
 * a selection publication, fresh salt and generation, never a plain refresh.
 */
export function planRenewPrivateSelection(records, command, now) {
  const scope = readRecord(records, command.scopeKey); const rollout = readRecord(records, scope.runningRolloutKey);
  const deployment = readRecord(records, rollout.deploymentKey); const release = readRecord(records, deployment.releaseKey);
  const proposal = command.publication; const head = readRecord(records, proposal.headKey);
  requireCondition(scope.state === 'active' && scope.hasCandidate === true && scope.metadataSyncPending === false
    && ['running', 'paused'].includes(rollout.state) && ['active', 'paused'].includes(deployment.state)
    && rollout.scopeKey === command.scopeKey && deployment.rolloutKey === scope.runningRolloutKey
    && proposal.mode === 'selection' && sameJson(proposal.affectedScopeKeys, [command.scopeKey])
    && proposal.privateReleaseUpdates?.length === 1 && proposal.privateReleaseUpdates[0].releaseKey === deployment.releaseKey,
  'MODEL_PRIVATE_RENEWAL_INVALID');
  const old = scope.currentCandidateOpening; const next = proposal.candidateOpenings?.find((item) => item.scopeKey === command.scopeKey)?.opening;
  requireCondition(old != null && next != null && old.deploymentId === deployment.deploymentId && old.deploymentRevision === deployment.revision
    && old.rolloutId === rollout.rolloutId && old.rolloutRevision === rollout.revision
    && old.releaseEnvelopeSha256 === currentReleaseHash(records, deployment.releaseKey)
    && next.releaseEnvelopeSha256 !== old.releaseEnvelopeSha256 && next.leafSalt !== old.leafSalt
    && sameJson({ ...old, releaseEnvelopeSha256: next.releaseEnvelopeSha256, leafSalt: next.leafSalt }, next)
    && proposal.privateReleaseUpdates[0].envelope.signed.revision > head.releaseEnvelopes[release.releaseId].signed.revision,
  'MODEL_PRIVATE_RENEWAL_INVALID');
  const parts = publicationParts(records, proposal, now);
  // Only the hidden Release reference/opening changes business selection.
  // Full public Release reference renewals still preserve immutable business.
  for (const target of proposal.bundle.members.filter((item) => item.signed.role === 'target')) {
    const previous = head.bundle.members.find((item) => item.signed.role === 'target' && sameJson(item.signed.scope, target.signed.scope));
    const withoutCommitment = (body) => { const { rolloutSetCommitment, selectionGeneration, ...rest } = targetSelection(body); return rest; };
    requireCondition(previous !== undefined && sameJson(withoutCommitment(previous.signed), withoutCommitment(target.signed)), 'MODEL_SELECTION_CHANGED');
  }
  const scopeWrite = parts.writes.find((write) => write.key === command.scopeKey);
  requireCondition(scopeWrite !== undefined && sameJson(parts.candidateOpenings.get(command.scopeKey), next), 'MODEL_OPENING_INVALID');
  scopeWrite.value.currentCandidateOpening = structuredClone(next);
  const viewOverrides = { [command.scopeKey]: scopeWrite.value,
    [proposal.headKey]: parts.writes.find((write) => write.key === proposal.headKey).value };
  const paths = pathReads(records, command.futurePathKey, rollout.deploymentKey, command.scopeKey, now,
    { phase: 'display', resuming: deployment.state === 'paused', viewOverrides });
  return commandPlan(records, command, now, [...new Set([...parts.reads, ...paths,
    command.scopeKey, scope.runningRolloutKey, rollout.deploymentKey, deployment.releaseKey])], parts.writes,
  [...parts.facts, { kind: 'private-release-renewed', releaseId: release.releaseId, committedAt: now,
    openingSha256: objectHash(next) }], { releaseId: release.releaseId, revision: proposal.privateReleaseUpdates[0].envelope.signed.revision });
}
