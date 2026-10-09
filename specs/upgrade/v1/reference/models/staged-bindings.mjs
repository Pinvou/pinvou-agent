import { requireCondition } from '../errors.mjs';
import { readRecord, selectionIdentitySha256 } from './inputs.mjs';

/** T19/T44 use the same protected waiting facts at both authorization issuance
 * and consumption. A later consume rejection cannot repair a stale validate.
 */
export function waitingStagedReads(records, key, session, preparation, claims, now) {
  requireCondition(key !== null && preparation.waitingStagedKey === key, 'MODEL_STAGED_INVALID');
  const staged = readRecord(records, key);
  const original = readRecord(records, staged.preinstallTransactionKey);
  const binding = claims.staged;
  requireCondition(binding !== null && staged.state === 'waiting' && staged.revision === session.stagedRevision
    && staged.revision === binding.stagedRevision && staged.preinstallTransactionId === binding.preinstallTransactionId
    && staged.preinstallState === 'staging_completed' && staged.slotIdentity === binding.slotIdentity
    && staged.stagedAt === binding.stagedAt && staged.stagedValidUntil === binding.stagedValidUntil
    && staged.channelRevision === claims.scope.channelRevision && now < staged.stagedValidUntil
    && staged.installationScopeId === claims.scope.installationScopeId
    && staged.selectionIdentitySha256 === selectionIdentitySha256(claims.update)
    && staged.observationWindowId === session.observationWindowId && staged.groupIdentity === session.groupIdentity
    && staged.groupIdentity === claims.update.originalStage.groupIdentity
    && original.transactionId === staged.preinstallTransactionId && original.executionPurpose === 'preinstall'
    && original.installationScopeId === staged.installationScopeId && original.channelRevision === staged.channelRevision
    && original.state === 'staging_completed' && original.slotIdentity === staged.slotIdentity
    && original.finalInstallerSha256 === claims.update.finalInstaller.sha256, 'MODEL_STAGED_INVALID');
  return [key, staged.preinstallTransactionKey];
}
