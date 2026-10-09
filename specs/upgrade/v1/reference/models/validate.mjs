import { requireCondition } from '../errors.mjs';
import { sameJson, DAY } from '../semantics.mjs';
import { verifyEnvelope } from '../signatures.mjs';
import { sha256 } from '../digests.mjs';
import { assertWorkerLease, atomicPlan, nextRecord } from './atomic.mjs';
import { commandPlan } from './operation.mjs';
import { readRecord, qualificationReads } from './inputs.mjs';
import { sessionTimeoutState } from './session.mjs';
import { scopeIndexWrite } from './scope-index.mjs';
import { workflowContext, hopIdentitySha256, workflowSessionReads } from './workflow.mjs';
import { currentRootBody } from './metadata-inputs.mjs';
import { waitingStagedReads } from './staged-bindings.mjs';

export function planBeginValidate(records, command, now) {
  const session = readRecord(records, command.sessionKey);
  const preparation = readRecord(records, command.preparationKey);
  const workflowReads = workflowSessionReads(records, session);
  const existing = records[command.operationKey];
  const operation = existing ?? { revision: 0, state: 'processing', requestDigest: command.requestDigest,
    ownerEpoch: 1, ownerStartedAt: now, lastRenewedAt: now, leaseExpiresAt: now + 30_000 };
  assertWorkerLease(operation, command.ownerEpoch, now);
  if (existing !== undefined) {
    requireCondition(operation.requestDigest === command.requestDigest && session.state === 'authorization_requested'
      && session.activeValidateKey === command.validateKey && session.futureActions === 'writable'
      && sessionTimeoutState(session, now) === session.state, 'MODEL_VALIDATE_CONFLICT');
    const active = readRecord(records, command.validateKey);
    requireCondition(active.state === 'processing' && active.operationKey === command.operationKey
      && active.preparationKey === command.preparationKey && preparation.state === 'preparing'
      && preparation.ownerEpoch === session.preparationEpoch, 'MODEL_VALIDATE_CONFLICT');
    const writes = active.ownerEpoch === operation.ownerEpoch ? [] : [{ key: command.validateKey,
      value: nextRecord(active, { ownerEpoch: operation.ownerEpoch }) }];
    return atomicPlan(records, [command.sessionKey, command.preparationKey, command.operationKey, command.validateKey, ...workflowReads], writes, [], now);
  }
  requireCondition(operation.requestDigest === command.requestDigest && session.state === 'preparation_ready'
    && session.futureActions === 'writable' && session.activeValidateKey === null
    && sessionTimeoutState(session, now) === session.state && preparation.state === 'preparing'
    && preparation.ownerEpoch === session.preparationEpoch
    && preparation.installationScopeId === session.installationScopeId, 'MODEL_VALIDATE_CONFLICT');
  return atomicPlan(records, [command.sessionKey, command.preparationKey, command.scopeIndexKey, ...workflowReads], [
    scopeIndexWrite(records, command.scopeIndexKey, session.installationScopeId, [command.validateKey]),
    { key: command.sessionKey, value: nextRecord(session, { state: 'authorization_requested', activeValidateKey: command.validateKey }) },
    { key: command.validateKey, value: { revision: 1, recordKind: 'activeValidate', installationScopeId: session.installationScopeId,
      state: 'processing', sessionId: session.sessionId,
      operationKey: command.operationKey, ownerEpoch: command.ownerEpoch, preparationKey: command.preparationKey } },
    { key: command.operationKey, value: nextRecord(operation, { phase: 'validate-coordinating' }) },
  ], [], now);
}

export function planFinishValidate(records, command, now) {
  const session = readRecord(records, command.sessionKey);
  const validate = readRecord(records, session.activeValidateKey);
  const preparation = readRecord(records, validate.preparationKey);
  const operation = readRecord(records, command.operationKey);
  assertWorkerLease(operation, command.ownerEpoch, now);
  requireCondition(validate.state === 'processing' && validate.operationKey === command.operationKey
    && validate.ownerEpoch === command.ownerEpoch && validate.sessionId === session.sessionId
    && session.state === 'authorization_requested' && session.futureActions === 'writable'
    && sessionTimeoutState(session, now) === session.state && preparation.state === 'preparing'
    && preparation.ownerEpoch === session.preparationEpoch, 'MODEL_VALIDATE_CONFLICT');
  const reads = [command.sessionKey, session.activeValidateKey, validate.preparationKey, command.rootKey];
  const writes = [{ key: session.activeValidateKey, value: nextRecord(validate, { state: command.result }) }];
  requireCondition(['authorized', 'authorization_failed', 'coordination_aborted'].includes(command.result), 'MODEL_VALIDATE_CONFLICT');
  if (command.result === 'authorized') {
    reads.push(...workflowSessionReads(records, session));
    reads.push(command.scopeIndexKey);
    writes.push(scopeIndexWrite(records, command.scopeIndexKey, session.installationScopeId, [command.authorizationKey]));
    const claims = verifyEnvelope(command.authorizationBytes, { trustedRoot: currentRootBody(records, command.rootKey),
      expected: command.expectedContext, now }).signed;
    reads.push(...qualificationReads(records, command.qualificationKey, now, command.expectedContext, 'T19', claims));
    requireCondition(claims.role === `authorization-${session.executionPurpose}`
      && claims.telemetrySessionId === session.sessionId && claims.installId === session.installId
      && claims.decisionId === session.decisionId && claims.decisionRevision === session.decisionRevision
      && claims.scope.installationScopeId === session.installationScopeId
      && claims.scope.channelRevision === session.channelRevision && claims.purpose === session.executionPurpose
      && claims.exp <= session.startedAt + DAY && claims.preparation.ownerEpoch === preparation.ownerEpoch
      && claims.preparation.scopeRevision === preparation.scopeRevision
      && sameJson(claims.freezeEpoch, preparation.freezeEpoch) && sameJson(claims.backup, preparation.backup)
      && claims.update.workflowId === session.workflowId && claims.update.finalInstaller.sha256 === session.finalInstallerSha256
      && sameJson(workflowContext(claims), session.immutableContext) && hopIdentitySha256(claims) === session.hopSha256,
    'MODEL_CONSUME_BINDING_INVALID');
    if (session.executionPurpose === 'activate') reads.push(...waitingStagedReads(records,
      preparation.waitingStagedKey, session, preparation, claims, now));
    writes.push({ key: command.authorizationKey, value: { revision: 1, recordKind: 'authorization',
      installationScopeId: session.installationScopeId, state: 'available',
      authorizationJti: claims.authorizationJti, sessionId: session.sessionId, sessionKey: command.sessionKey,
      expiresAt: claims.exp, envelopeSha256: sha256(command.authorizationBytes), context: command.expectedContext,
      claims, consumeReservationKey: null } });
  }
  writes.push({ key: command.sessionKey, value: nextRecord(session, { state: command.result, activeValidateKey: null,
    authorizationKey: command.result === 'authorized' ? command.authorizationKey : null,
    futureActions: command.result === 'authorized' ? 'writable' : 'fenced' }) });
  return commandPlan(records, command, now, [...new Set(reads)], writes, [], { state: command.result });
}
