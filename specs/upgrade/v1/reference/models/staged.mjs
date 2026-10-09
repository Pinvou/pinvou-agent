import { requireCondition } from '../errors.mjs';
import { DAY, sameJson } from '../semantics.mjs';
import { nextRecord } from './atomic.mjs';
import { commandPlan } from './operation.mjs';
import { readRecord, qualificationReads, selectionIdentitySha256 } from './inputs.mjs';
import { assertTransactionTransition } from './transaction.mjs';
import { scopeIndexWrite, scopeOccupancyReads } from './scope-index.mjs';

export function planCompletePreinstall(records, command, now) {
  const transaction = readRecord(records, command.transactionKey);
  const proof = readRecord(records, command.slotProofKey);
  const channel = readRecord(records, command.channelKey);
  const preparation = readRecord(records, command.preparationKey);
  const qualification = readRecord(records, command.qualificationKey);
  const qualifiedReads = qualificationReads(records, command.qualificationKey, now, command.context, 'T19', null, transaction.immutableUpdate);
  requireCondition(transaction.executionPurpose === 'preinstall' && transaction.state === 'staging_verified'
    && transaction.channelRevision === channel.channelRevision && preparation.state === 'executing'
    && transaction.installationScopeId === preparation.installationScopeId
    && transaction.installationScopeId === channel.installationScopeId
    && preparation.transactionId === transaction.transactionId && preparation.ownerEpoch === transaction.ownerEpoch
    && proof.ownerEpoch === transaction.ownerEpoch && proof.activeWriterCount === 0
    && proof.allExecutorsStopped === true && preparation.waitingStagedKey === null, 'MODEL_STAGED_INVALID');
  requireCondition(transaction.safeCancellationRequested !== true
    && qualification.selectionIdentitySha256 === selectionIdentitySha256(transaction.immutableUpdate), 'MODEL_STAGED_INVALID');
  assertTransactionTransition(transaction, 'staging_completed', { now, facts: proof });
  const end = transaction.immutableUpdate.hopChain.installNotAfter;
  const stagedValidUntil = Math.min(now + 30 * DAY, end ?? Infinity);
  requireCondition(now < stagedValidUntil, 'MODEL_STAGED_INVALID');
  const staged = { revision: 1, recordKind: 'staged', state: 'waiting', installationScopeId: transaction.installationScopeId,
    channelRevision: channel.channelRevision, preinstallTransactionId: transaction.transactionId,
    preinstallTransactionKey: command.transactionKey, selectionIdentitySha256: selectionIdentitySha256(transaction.immutableUpdate),
    observationWindowId: transaction.observationWindowId, groupIdentity: transaction.groupIdentity,
    preinstallState: 'staging_completed', slotIdentity: transaction.slotIdentity, stagedAt: now, stagedValidUntil,
    immutableUpdate: structuredClone(transaction.immutableUpdate), activationTransactionId: null };
  return commandPlan(records, command, now, [...new Set([command.transactionKey, command.slotProofKey, command.channelKey, command.preparationKey,
    command.scopeIndexKey, ...qualifiedReads,
    ...scopeOccupancyReads(records, command.scopeIndexKey, transaction.installationScopeId, 'waiting')])], [
    scopeIndexWrite(records, command.scopeIndexKey, transaction.installationScopeId, [command.stagedKey]),
    { key: command.transactionKey, value: nextRecord(transaction, { state: 'staging_completed' }) },
    { key: command.stagedKey, value: staged },
    { key: command.preparationKey, value: nextRecord(preparation, { state: 'idle', transactionId: null,
      waitingStagedKey: command.stagedKey }) },
  ], [], { stagedKey: command.stagedKey, stagedRevision: 1 });
}

export function planRebuildWaiting(records, command, now) {
  const staged = readRecord(records, command.stagedKey);
  const transaction = readRecord(records, command.cancelledTransactionKey);
  const proof = readRecord(records, command.safetyProofKey);
  const channel = readRecord(records, command.channelKey);
  const preparation = readRecord(records, command.preparationKey);
  requireCondition(readRecord(records, command.qualificationKey).selectionIdentitySha256 === selectionIdentitySha256(staged.immutableUpdate),
    'MODEL_STAGED_INVALID');
  requireCondition(staged.state === 'activating' && transaction.executionPurpose === 'activate'
    && transaction.state === 'cancelled_before_install' && staged.activationTransactionId === transaction.transactionId
    && transaction.boundaryAt === null && proof.boundaryAt === null && proof.activeMutationCount === 0
    && proof.activeWriterCount === 0 && proof.allExecutorsStopped === true && proof.stoppedOwnerEpoch === transaction.ownerEpoch
    && proof.oldIdentitySha256 === transaction.oldIdentitySha256 && proof.oldDataSha256 === transaction.oldDataSha256
    && proof.verifiedSlotId === staged.slotIdentity && proof.verifiedObjectSha256 === transaction.finalInstallerSha256
    && now < staged.stagedValidUntil && channel.channelRevision === staged.channelRevision
    && staged.installationScopeId === transaction.installationScopeId
    && transaction.installationScopeId === preparation.installationScopeId
    && transaction.installationScopeId === channel.installationScopeId
    && preparation.ownerEpoch === transaction.ownerEpoch && preparation.waitingStagedKey === null
    && sameJson(staged.immutableUpdate, transaction.immutableUpdate)
    && preparation.transactionId === transaction.transactionId && preparation.state === 'executing', 'MODEL_STAGED_INVALID');
  const reads = [command.stagedKey, command.cancelledTransactionKey, command.safetyProofKey, command.channelKey,
    command.preparationKey, command.scopeIndexKey,
    ...scopeOccupancyReads(records, command.scopeIndexKey, transaction.installationScopeId, 'waiting'),
    ...qualificationReads(records, command.qualificationKey, now, command.context, 'T19', null, staged.immutableUpdate)];
  const waiting = { ...structuredClone(staged), revision: staged.revision + 1,
    state: 'waiting', activationTransactionId: null, cancelledActivationTransactionId: transaction.transactionId };
  return commandPlan(records, command, now, [...new Set(reads)], [
    scopeIndexWrite(records, command.scopeIndexKey, transaction.installationScopeId, [command.newWaitingKey]),
    { key: command.stagedKey, value: nextRecord(staged, { state: 'cancelled', successorWaitingKey: command.newWaitingKey }) },
    { key: command.newWaitingKey, value: waiting },
    { key: command.preparationKey, value: nextRecord(preparation, { state: 'idle', transactionId: null,
      waitingStagedKey: command.newWaitingKey }) },
  ], [], { stagedKey: command.newWaitingKey, stagedRevision: waiting.revision });
}
