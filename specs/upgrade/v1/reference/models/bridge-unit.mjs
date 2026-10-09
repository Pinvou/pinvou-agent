import { requireCondition } from '../errors.mjs';
import { assertApiShape } from '../api-registry.mjs';
import { nextRecord } from './atomic.mjs';
import { readRecord, objectHash } from './inputs.mjs';
import { approvalReads, approvalContext } from './entities.mjs';

/** T13's prepared bridge unit is jointly committed with the old baseline's
 * supersession. It cannot enable a bridge before that parent transition.
 * The approval binds the actual resulting parent/eligibility revisions.
 */
export function bridgeUnitParts(records, key, scope, oldBaselineKey, now) {
  const unit = readRecord(records, key); assertApiShape('prepared-bridge', unit);
  const parent = readRecord(records, oldBaselineKey); const eligibility = readRecord(records, unit.eligibilityKey);
  requireCondition(unit.deploymentKey === oldBaselineKey && parent.state === 'active'
    && unit.expectedDeploymentRevision === parent.revision && unit.expectedEligibilityRevision === eligibility.revision
    && eligibility.recordKind === 'bridgeEligibility' && eligibility.state === 'disabled'
    && eligibility.deploymentKey === oldBaselineKey && eligibility.channel === scope.channel
    && unit.validFrom <= now && (unit.expiresAt === null || now < unit.expiresAt), 'MODEL_BRIDGE_INELIGIBLE');
  const body = { command: 'EnableBridge', deploymentId: parent.deploymentId, resultingDeploymentRevision: parent.revision + 1,
    eligibilityId: eligibility.eligibilityId, resultingEligibilityRevision: eligibility.revision + 1,
    validFrom: unit.validFrom, expiresAt: unit.expiresAt };
  const reads = [key, oldBaselineKey, unit.eligibilityKey, ...approvalReads(records, unit.approvalKey, objectHash(body),
    scope.channel === 'stable' ? 2 : 1, approvalContext('EnableBridge', scope, eligibility.eligibilityId, eligibility.revision))];
  return { reads, writes: [{ key: unit.eligibilityKey, value: nextRecord(eligibility, { state: 'enabled',
    deploymentRevision: parent.revision + 1, validFrom: unit.validFrom, expiresAt: unit.expiresAt }) }] };
}
