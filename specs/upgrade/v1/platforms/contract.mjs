import { checkContext, shape, id, oneOf, requireFact, PlatformContractError } from './values.mjs';
import { identityOperations } from './identity.mjs';
import { ownershipOperations } from './ownership.mjs';
import { timeOperations } from './time.mjs';
import { executionOperations } from './execution.mjs';
import { backupOperations } from '../backup/operations.mjs';
import { storageOperations } from './storage.mjs';

export const revision = 'pinvou-platform-observations-v1';
export const operations = Object.freeze({ ...identityOperations, ...ownershipOperations, ...timeOperations,
  ...executionOperations, ...backupOperations, ...storageOperations });
const failures = Object.freeze({
  unsupported: ['not_implemented', 'target_not_supported'],
  prerequisite_unavailable: ['qualification_missing', 'owner_missing', 'registration_missing', 'duration_missing'],
  refused: ['permission_denied', 'interaction_required', 'stale_owner', 'incomplete_scope', 'identity_mismatch',
    'consistency_unproven', 'budget_exhausted', 'invalid_format', 'safety_unproven'],
  unavailable: ['dependency_unavailable'],
  time_unknown: ['continuity_unproven', 'anchor_missing', 'remaining_unproven'],
  confirmed_fault: ['positively_observed_fault'],
  outcome_unknown: ['commit_unconfirmed', 'action_unconfirmed'],
});

/** Check an adapter's observation record, not its truth or authority. A passing
 * record is ordinary DATA; callers must retain the actual native capability.
 * Invoke with bounded parsed JSON (parseJson), never getters or live objects.
 */
export function checkObservation(record) {
  requireFact(shape({ operation: id, context: value => value !== null && typeof value === 'object',
    request: value => value !== null && typeof value === 'object', result: value => value !== null && typeof value === 'object' })(record), 'PLATFORM_RECORD_INVALID');
  requireFact(Object.hasOwn(operations, record.operation), 'PLATFORM_OPERATION_UNKNOWN');
  checkContext(record.context);
  const definition = operations[record.operation], { request, result } = record;
  requireFact(definition.request(request), 'PLATFORM_REQUEST_INVALID');
  if (result.kind === 'observed') {
    requireFact(shape({ kind: oneOf('observed'), effect: oneOf(definition.effect), facts: definition.facts })(result), 'PLATFORM_RESULT_INVALID');
    definition.check(request, result.facts, record.context);
  } else {
    requireFact(Object.hasOwn(failures, result.kind) && shape({ kind: oneOf(result.kind),
      effect: oneOf(result.kind === 'outcome_unknown' ? 'possible' : 'none'),
      reason: oneOf(...failures[result.kind]), faultEvidenceId: value => result.kind === 'confirmed_fault' ? id(value) : value === null })(result),
    'PLATFORM_RESULT_INVALID');
    if (result.kind === 'outcome_unknown') requireFact(definition.effect === 'committed', 'PLATFORM_EFFECT_INVALID');
  }
  return structuredClone(record); // A defensive DATA copy, never a native seal.
}
export function unsupported(context, operation, request) {
  return checkObservation({ context, operation, request,
    result: { kind: 'unsupported', effect: 'none', reason: 'not_implemented', faultEvidenceId: null } });
}
export { PlatformContractError };
