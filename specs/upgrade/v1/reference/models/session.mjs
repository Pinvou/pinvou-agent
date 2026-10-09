import { requireCondition } from '../errors.mjs';
import { SESSION_EDGES, SESSION_TERMINALS, assertEdge } from './state-graphs.mjs';
import { assertFields } from './inputs.mjs';

const automatic = new Set(['silent', 'forced']);
const serverResults = new Set(['authorized', 'authorization_failed', 'coordination_aborted',
  'authorization_consumed', 'cancelled_before_install', 'authorization_expired']);
const knownStates = new Set([...Object.keys(SESSION_EDGES), ...Object.values(SESSION_EDGES).flat(), ...SESSION_TERMINALS]);

/** Session purpose is execution purpose; telemetry credential purpose remains
 * telemetry. Facts below come from the protected helper/current service read set.
 */
export function assertSessionTransition(session, to, { now, actor, facts }) {
  if (session.updateAvailable === false) {
    requireCondition(session.state === 'decision_received' && to === 'no_update'
      && Number.isSafeInteger(now) && now >= session.startedAt && now < session.startedAt + 86_400_000,
    'MODEL_EDGE_INVALID');
    return;
  }
  assertFields(session, { state: 'id', startedAt: 'integer', sessionId: 'id', workflowId: 'id',
    hopSha256: 'hash', installationScopeId: 'id', preparationEpoch: 'positive',
    freezeEpoch: 'nullable-integer', dataScopeSha256: 'hash', finalInstallerSha256: 'hash', backupPolicy: ['required', 'notRequired'] });
  requireCondition(knownStates.has(session.state), 'MODEL_EDGE_INVALID');
  requireCondition(session.executionPurpose !== 'activate' || !['user_confirmed', 'download_resume_context', 'download_started',
    'download_succeeded', 'download_failed', 'verification_started', 'verification_succeeded', 'verification_failed'].includes(session.state), 'MODEL_PURPOSE_INVALID');
  requireCondition(['preinstall', 'install', 'activate'].includes(session.executionPurpose)
    && ['normal', 'silent', 'forced'].includes(session.upgradeType)
    && (session.executionPurpose === 'install' ? session.upgradeType !== 'silent' : session.upgradeType === 'silent'),
    'MODEL_PURPOSE_INVALID');
  requireCondition(!SESSION_TERMINALS.includes(session.state), 'MODEL_TERMINAL_IMMUTABLE');
  requireCondition(Number.isSafeInteger(now) && now >= session.startedAt
    && Number.isSafeInteger(session.startedAt + 86_400_000) && now < session.startedAt + 86_400_000, 'MODEL_SESSION_EXPIRED');
  requireCondition(!serverResults.has(to) || actor === 'lifecycle', 'MODEL_SERVER_ONLY');
  if (['cancelled', 'expired', 'channel_changed'].includes(to)) {
    requireCondition(actor === 'lifecycle' && facts.cause === to && session.state !== 'authorized', 'MODEL_CANCEL_INVALID');
    // The ordinary transition API cannot fabricate timeout or channel facts.
    // Those two outcomes are committed only by their atomic lifecycle commands.
    requireCondition(to === 'cancelled' && facts.consumeState === 'not_sent', 'MODEL_CANCEL_INVALID');
    return;
  }
  assertEdge(SESSION_EDGES, session.state, to);
  const purpose = session.executionPurpose;
  if (to === 'user_confirmed') requireCondition(session.upgradeType === 'normal'
    && facts.confirmedHopSha256 === session.hopSha256, 'MODEL_CONFIRMATION_REQUIRED');
  if (to === 'download_started') {
    requireCondition(purpose !== 'activate', 'MODEL_PURPOSE_INVALID');
    if (['update_offered', 'deferred'].includes(session.state)) {
      requireCondition(automatic.has(session.upgradeType), 'MODEL_CONFIRMATION_REQUIRED');
    }
  }
  if (to === 'download_resume_context') requireCondition(purpose !== 'activate'
    && session.downloadResumeBinding != null
    && session.downloadResumeBinding.workflowId === session.workflowId
    && session.downloadResumeBinding.toSessionId === session.sessionId
    && session.downloadResumeBinding.fromSessionId !== session.sessionId
    && session.downloadResumeBinding.hopSha256 === session.hopSha256
    && session.downloadResumeBinding.immutableContextSha256 === session.immutableContextSha256
    && Number.isSafeInteger(session.downloadResumeBinding.handedOffAt)
    && session.downloadResumeBinding.handedOffAt <= now
    && facts.workflowId === session.workflowId && facts.writableSessionId === session.sessionId
    && facts.hopSha256 === session.hopSha256
    && (session.upgradeType !== 'normal' || facts.confirmedHopSha256 === session.hopSha256
      && session.downloadResumeBinding.confirmedHopSha256 === session.hopSha256), 'MODEL_RESUME_INVALID');
  if (to === 'staged_verified') requireCondition(purpose === 'activate'
    && facts.trigger === 'restart' && facts.preinstallState === 'staging_completed'
    && facts.stagedRevision === session.stagedRevision && facts.hopSha256 === session.hopSha256
    && now < facts.stagedValidUntil, 'MODEL_STAGED_INVALID');
  if (to === 'preparation_started') requireCondition(purpose !== 'preinstall', 'MODEL_PREINSTALL_ACTIVITY_FORBIDDEN');
  if (to === 'writers_frozen') requireCondition(purpose !== 'preinstall'
    && facts.ownerEpoch === session.preparationEpoch && facts.writerCount === 0
    && Number.isSafeInteger(facts.freezeEpoch) && facts.freezeEpoch > 0
    && facts.frozenDataScopeSha256 === session.dataScopeSha256, 'MODEL_FREEZE_INVALID');
  if (to === 'backup_started' || to === 'backup_succeeded') requireCondition(purpose !== 'preinstall'
    && session.backupPolicy === 'required' && Number.isSafeInteger(session.freezeEpoch)
    && session.freezeEpoch > 0, 'MODEL_BACKUP_FORBIDDEN');
  if (to === 'backup_succeeded') requireCondition(facts.backupFreezeEpoch === session.freezeEpoch
    && facts.backupPreparationEpoch === session.preparationEpoch
    && facts.backupDataScopeSha256 === session.dataScopeSha256, 'MODEL_BACKUP_INVALID');
  if (to === 'preparation_ready') {
    const expected = purpose === 'preinstall' ? 'permission_granted'
      : session.backupPolicy === 'required' ? 'backup_succeeded' : 'writers_frozen';
    requireCondition(session.state === expected && facts.ownerEpoch === session.preparationEpoch, 'MODEL_PREPARATION_INVALID');
  }
  if (to === 'verification_succeeded') requireCondition(facts.verifiedObjectSha256 === session.finalInstallerSha256
    && facts.verificationResult === 'verified', 'MODEL_VERIFICATION_INVALID');
  if (to === 'preflight_succeeded') requireCondition(facts.preflightResult === 'supported'
    && facts.targetScopeId === session.installationScopeId, 'MODEL_PREFLIGHT_INVALID');
  if (to === 'permission_granted') requireCondition(facts.permissionScopeId === session.installationScopeId
    && (session.upgradeType !== 'silent' || facts.permissionSource === 'existing-helper'), 'MODEL_PERMISSION_INVALID');
}

export function sessionTimeoutState(session, now) {
  assertFields(session, { state: 'id', startedAt: 'integer' });
  requireCondition(Number.isSafeInteger(now) && now >= session.startedAt
    && Number.isSafeInteger(session.startedAt + 86_400_000), 'TRUSTED_TIME_REQUIRED');
  if (SESSION_TERMINALS.includes(session.state) || now < session.startedAt + 86_400_000) return session.state;
  return session.state === 'authorized' ? 'authorization_expired' : 'expired';
}
