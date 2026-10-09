import { requireCondition } from '../errors.mjs';
import { readRecord, objectHash, assertIdentity } from './inputs.mjs';
import { approvalReads, approvalContext } from './entities.mjs';

/** T14 supplies the exact disposition of the original pause. A previous plan
 * approval cannot approve a new restoration of eligibility.
 */
export function resumptionReads(records, command, deploymentKey, scope) {
  const deployment = readRecord(records, deploymentKey);
  const resolution = readRecord(records, command.pauseResolutionKey);
  assertIdentity(resolution.reason);
  requireCondition(deployment.pauseReason !== undefined && resolution.projectionOwner === 'T14' && resolution.state === 'resolved'
    && resolution.deploymentKey === deploymentKey && resolution.deploymentRevision === deployment.revision
    && resolution.pauseReasonSha256 === objectHash(deployment.pauseReason), 'MODEL_PAUSE_UNRESOLVED');
  const body = { command: 'ResumeDeployment', deploymentKey, deploymentRevision: deployment.revision,
    pauseReasonSha256: resolution.pauseReasonSha256, resolutionSha256: objectHash(resolution) };
  return [command.pauseResolutionKey, ...approvalReads(records, command.resumeApprovalKey, objectHash(body),
    scope.channel === 'stable' ? 2 : 1, approvalContext('ResumeDeployment', scope, deploymentKey, deployment.revision))];
}
