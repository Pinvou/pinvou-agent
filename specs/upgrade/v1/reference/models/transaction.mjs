import { requireCondition } from '../errors.mjs';
import { TRANSACTION_EDGES, TRANSACTION_TERMINALS, assertEdge } from './state-graphs.mjs';
import { assertFields, assertInteger } from './inputs.mjs';

/** Boundaries are protected observations, never inferred from a process name.
 * A platform installer invocation always crosses install's execution boundary.
 */
export function assertTransactionTransition(transaction, to, { now, facts }) {
  assertFields(transaction, { state: 'id', startedAt: 'integer', ownerEpoch: 'positive',
    authorizationJti: 'id', finalInstallerSha256: 'hash', dataScopeSha256: 'hash',
    oldIdentitySha256: 'hash', oldDataSha256: 'hash', boundaryAt: 'nullable-integer' });
  requireCondition(Object.hasOwn(transaction, 'helperPlanRef') && Object.hasOwn(transaction, 'slotIdentity'), 'MODEL_INPUT_INVALID');
  const { executionPurpose: purpose } = transaction;
  requireCondition(Object.hasOwn(TRANSACTION_EDGES, purpose), 'MODEL_PURPOSE_INVALID');
  const known = new Set([...Object.keys(TRANSACTION_EDGES[purpose]), ...Object.values(TRANSACTION_EDGES[purpose]).flat()]);
  requireCondition(known.has(transaction.state) || TRANSACTION_TERMINALS.includes(transaction.state), 'MODEL_EDGE_INVALID');
  requireCondition(!TRANSACTION_TERMINALS.includes(transaction.state), 'MODEL_TERMINAL_IMMUTABLE');
  const crossed = ['installer_started', 'activation_started', 'reconciling', 'installation_verified', 'health_check_started'].includes(transaction.state);
  requireCondition(purpose === 'preinstall' ? transaction.boundaryAt === null
    : crossed ? transaction.boundaryAt !== null && transaction.boundaryAt >= transaction.startedAt && transaction.boundaryAt <= now
      : transaction.boundaryAt === null, 'MODEL_BOUNDARY_INVALID');
  requireCondition(Number.isSafeInteger(now) && now >= transaction.startedAt
    && Number.isSafeInteger(transaction.startedAt + 30 * 86_400_000)
    && now < transaction.startedAt + 30 * 86_400_000, 'MODEL_TRANSACTION_EXPIRED');
  const safeStop = facts.stoppedOwnerEpoch === transaction.ownerEpoch && facts.activeWriterCount === 0
    && facts.allExecutorsStopped === true;
  if (purpose === 'preinstall' && ['staging_failed', 'staging_cancelled'].includes(to)) {
    requireCondition(safeStop && facts.isolatedSlotId === transaction.slotIdentity, 'MODEL_STOP_UNPROVEN');
    return;
  }
  if (to === 'abandoned_before_install' || to === 'cancelled_before_install') {
    requireCondition(purpose !== 'preinstall'
      && to === (purpose === 'install' ? 'abandoned_before_install' : 'cancelled_before_install')
      && transaction.boundaryAt === null && facts.boundaryAt === null && safeStop
      && facts.activeMutationCount === 0
      && facts.oldIdentitySha256 === transaction.oldIdentitySha256
      && facts.oldDataSha256 === transaction.oldDataSha256, 'MODEL_STOP_UNPROVEN');
    return;
  }
  if (to === 'failed_manual_repair_required') {
    requireCondition(purpose !== 'preinstall' && (transaction.boundaryAt !== null || facts.safetyResult === 'unproven'
      || facts.safetyResult === 'damaged'),
    'MODEL_FAILURE_UNPROVEN');
    return;
  }
  assertEdge(TRANSACTION_EDGES[purpose], transaction.state, to);
  if (to === 'helper_plan_started') requireCondition(transaction.helperPlanRef !== null, 'MODEL_PLAN_MISSING');
  if (purpose === 'install' && transaction.state === 'authorization_consumed' && to === 'execution_ready') {
    requireCondition(transaction.helperPlanRef === null, 'MODEL_PLAN_SKIPPED');
  }
  if (['installer_started', 'activation_started'].includes(to)) {
    requireCondition(facts.lastOnlineAuthorizationJti === transaction.authorizationJti
      && facts.lastOnlineAt === now && facts.verifiedObjectSha256 === transaction.finalInstallerSha256,
    'MODEL_FINAL_REVIEW_REQUIRED');
    const boundary = purpose === 'install' ? facts.installerInvokedAt
      : Math.min(facts.firstActiveWriteAt ?? Infinity, facts.pointerSwitchedAt ?? Infinity);
    requireCondition(Number.isSafeInteger(boundary) && boundary === now, 'MODEL_BOUNDARY_INVALID');
  }
  if (to === 'staging_verified' || to === 'staging_completed') requireCondition(facts.verifiedSlotId === transaction.slotIdentity
    && facts.verifiedObjectSha256 === transaction.finalInstallerSha256 && facts.activeMutationCount === 0, 'MODEL_SLOT_INVALID');
  if (to === 'installation_verified') requireCondition(facts.activeObjectSha256 === transaction.finalInstallerSha256
    && facts.dataScopeSha256 === transaction.dataScopeSha256, 'MODEL_ACTIVE_IDENTITY_INVALID');
  if (to === 'health_check_started') requireCondition(Number.isSafeInteger(facts.healthStartedAt)
    && facts.healthStartedAt >= transaction.boundaryAt && facts.healthStartedAt <= now, 'MODEL_HEALTH_INVALID');
  if (to === 'succeeded') requireCondition(facts.activeObjectSha256 === transaction.finalInstallerSha256
    && facts.healthResult === 'passed' && facts.dataResult === 'consistent'
    && Number.isSafeInteger(transaction.healthStartedAt) && facts.healthStartedAt === transaction.healthStartedAt
    && facts.healthStartedAt >= transaction.boundaryAt
    && facts.healthStartedAt <= now && now - facts.healthStartedAt <= 300_000
    && Number.isSafeInteger(facts.healthAttempts) && facts.healthAttempts >= 1 && facts.healthAttempts <= 2
    && Number.isSafeInteger(facts.lastHealthAttemptMs) && facts.lastHealthAttemptMs >= 0
    && facts.lastHealthAttemptMs <= 120_000, 'MODEL_HEALTH_INVALID');
}

export function transactionTimeoutState(transaction, now) {
  assertFields(transaction, { state: 'id', startedAt: 'integer', executionPurpose: ['preinstall', 'install', 'activate'] });
  requireCondition(Number.isSafeInteger(now) && now >= transaction.startedAt, 'TRUSTED_TIME_REQUIRED');
  requireCondition(Number.isSafeInteger(transaction.startedAt + 30 * 86_400_000), 'TRUSTED_TIME_REQUIRED');
  if (TRANSACTION_TERMINALS.includes(transaction.state) || now < transaction.startedAt + 30 * 86_400_000) return transaction.state;
  return transaction.executionPurpose === 'preinstall' ? 'staging_failed' : 'failed_manual_repair_required';
}

/** Boundary facts are persisted together with the edge so later cancellation
 * can never mistake an invoked installer or an early data write for readiness.
 */
export function transactionTransition(transaction, to, input) {
  assertInteger(transaction.revision);
  assertTransactionTransition(transaction, to, input);
  const boundaryAt = ['installer_started', 'activation_started'].includes(to) ? input.now : transaction.boundaryAt;
  return { ...structuredClone(transaction), revision: transaction.revision + 1, state: to, boundaryAt,
    healthStartedAt: to === 'health_check_started' ? input.facts.healthStartedAt : (transaction.healthStartedAt ?? null) };
}
