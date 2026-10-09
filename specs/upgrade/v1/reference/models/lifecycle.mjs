import { sha256, statusBindingHash, consumeRequestDigest, recoveryHandleHash, equalDigest } from '../digests.mjs';
import { requireCondition } from '../errors.mjs';
import { sameJson, DAY, consumeQueryAvailable } from '../semantics.mjs';
import { assertConsumeBinding } from '../signatures.mjs';
import { parseJson } from '../canonical-json.mjs';
import { assertApiShape } from '../api-registry.mjs';
import { atomicPlan, nextRecord } from './atomic.mjs';
import { commandPlan } from './operation.mjs';
import { readRecord, assertFields, qualificationReads } from './inputs.mjs';
import { sessionTimeoutState } from './session.mjs';
import { transactionTimeoutState } from './transaction.mjs';
import { scopeIndexWrite, scopeOccupancyReads } from './scope-index.mjs';
import { contributionWrite } from './contributions.mjs';
import { currentRootBody } from './metadata-inputs.mjs';
import { waitingStagedReads } from './staged-bindings.mjs';
import { workflowSessionReads } from './workflow.mjs';

function authorizationRecords(records, command) {
  const authorization = readRecord(records, command.authorizationKey);
  const session = readRecord(records, command.sessionKey);
  assertFields(authorization, { authorizationJti: 'id', sessionId: 'id', expiresAt: 'integer',
    revision: 'integer', envelopeSha256: 'hash', state: ['available', 'consumed', 'cancelled', 'expired'] });
  requireCondition(authorization.sessionKey === command.sessionKey && session.authorizationKey === command.authorizationKey
    && authorization.sessionId === session.sessionId, 'MODEL_CONSUME_BINDING_INVALID');
  return { authorization, session };
}

/** Durable server-owned recovery context contains no recovery secret. A
 * reservation precedes consumption; its outcome shares the terminal commit.
 */
function reservation(records, authorization) {
  if (authorization.consumeReservationKey === null) return null;
  const key = authorization.consumeReservationKey; const record = readRecord(records, key);
  const request = record.request; const { consumeRequestDigest: digest, ...semantic } = request;
  requireCondition(consumeRequestDigest(semantic) === digest
    && record.authorizationJti === authorization.authorizationJti
    && request.authorizationJti === authorization.authorizationJti
    && request.authorizationEnvelopeSha256 === authorization.envelopeSha256
    && request.product === authorization.claims.product && request.component === authorization.claims.component
    && request.installationScopeId === authorization.claims.scope.installationScopeId
    && request.purpose === authorization.claims.purpose && request.transactionId === authorization.claims.transactionId,
  'MODEL_CONSUME_BINDING_INVALID');
  return { key, record, request };
}
function outcomeValue(authorization, request, state, now) {
  const binding = { recoveryHandleHash: request.recoveryHandleHash, product: request.product,
    component: request.component, installationScopeId: request.installationScopeId,
    purpose: request.purpose, authorizationJti: request.authorizationJti, consumeKey: request.consumeKey,
    transactionId: request.transactionId, consumeRequestDigest: request.consumeRequestDigest };
  return { revision: 1, state, authorizationJti: request.authorizationJti,
    transactionId: state === 'consumed' ? request.transactionId : null,
    authorizationExp: authorization.expiresAt, committedAt: now,
    recoveryHandleHash: request.recoveryHandleHash, statusBindingHash: statusBindingHash(binding) };
}
export function authorizationEndWrites(records, authorizationKey, authorization, state, now) {
  const binding = reservation(records, authorization);
  const writes = [{ key: authorizationKey, value: nextRecord(authorization, { state, endedAt: now }) }];
  if (binding !== null) {
    requireCondition(!Object.hasOwn(records, binding.record.outcomeKey), 'MODEL_EXECUTION_DUPLICATE');
    writes.push({ key: binding.record.outcomeKey, value: outcomeValue(authorization, binding.request, state, now) });
  }
  return { reads: binding === null ? [] : [binding.key], writes };
}

export function planConsume(records, command, now) {
  const { authorizationKey, sessionKey, transactionKey, preparationKey, qualificationKey,
    rootKey, stagedKey = null, expectedAuthorizationRevision, request, authorizationBytes } = command;
  const { authorization, session } = authorizationRecords(records, command);
  requireCondition(authorization.revision === expectedAuthorizationRevision && authorization.state === 'available'
    && session.state === 'authorized' && session.futureActions === 'writable', 'MODEL_AUTHORIZATION_CONFLICT');
  requireCondition(now < authorization.expiresAt && sessionTimeoutState(session, now) === 'authorized', 'MODEL_AUTHORIZATION_EXPIRED');
  const binding = reservation(records, authorization);
  requireCondition(binding !== null && sameJson(binding.request, request)
    && binding.record.transactionKey === transactionKey, 'MODEL_CONSUME_BINDING_INVALID');
  const { consumeRequestDigest: digest, ...semantic } = request;
  requireCondition(consumeRequestDigest(semantic) === digest, 'MODEL_CONSUME_BINDING_INVALID');
  assertConsumeBinding(semantic, authorizationBytes, { trustedRoot: currentRootBody(records, rootKey),
    expected: authorization.context, now });
  requireCondition(sha256(authorizationBytes) === authorization.envelopeSha256
    && sameJson(parseJson(authorizationBytes).signed, authorization.claims) && session.executionPurpose === request.purpose,
  'MODEL_CONSUME_BINDING_INVALID');
  const preparation = readRecord(records, preparationKey);
  assertFields(preparation, { state: ['preparing'], ownerEpoch: 'positive', scopeRevision: 'positive',
    installationScopeId: 'id', dataScopeSha256: 'hash', oldIdentitySha256: 'hash', oldDataSha256: 'hash' });
  requireCondition(preparation.ownerEpoch === request.preparation.ownerEpoch
    && preparation.scopeRevision === request.preparation.scopeRevision
    && preparation.installationScopeId === request.installationScopeId
    && sameJson(preparation.freezeEpoch, request.freezeEpoch) && sameJson(preparation.backup, request.backup), 'MODEL_OWNER_FENCED');
  const reads = [authorizationKey, sessionKey, preparationKey, rootKey, binding.key, command.scopeIndexKey, session.contributionHeadKey,
    ...workflowSessionReads(records, session),
    ...scopeOccupancyReads(records, command.scopeIndexKey, request.installationScopeId, 'transaction'),
    ...qualificationReads(records, qualificationKey, now, authorization.context, 'T19', authorization.claims)];
  const claims = authorization.claims;
  const transaction = { revision: 1, recordKind: 'transaction', state: 'authorization_consumed', executionPurpose: request.purpose,
    transactionId: request.transactionId, authorizationJti: request.authorizationJti, startedAt: now,
    installId: claims.installId,
    ownerEpoch: preparation.ownerEpoch, installationScopeId: request.installationScopeId,
    helperPlanRef: claims.update.plans.helperMigration, finalInstallerSha256: claims.update.finalInstaller.sha256,
    dataScopeSha256: preparation.dataScopeSha256, oldIdentitySha256: preparation.oldIdentitySha256,
    oldDataSha256: preparation.oldDataSha256, boundaryAt: null,
    slotIdentity: request.preinstallSlot?.slotIdentity ?? request.staged?.slotIdentity ?? null,
    workflowId: claims.update.workflowId, immutableUpdate: structuredClone(claims.update), contributionHeadKey: session.contributionHeadKey,
    observationWindowId: session.observationWindowId, groupIdentity: claims.update.originalStage.groupIdentity,
    channelRevision: claims.scope.channelRevision, freezeEpoch: request.freezeEpoch, backup: structuredClone(request.backup),
    sequence: 0, timeoutAt: now + 30 * DAY, diagnosticCursorKey: binding.record.diagnosticCursorKey };
  const writes = [
    contributionWrite(records, session.contributionHeadKey, { windowId: session.observationWindowId, groupIdentity: claims.update.originalStage.groupIdentity }),
    scopeIndexWrite(records, command.scopeIndexKey, request.installationScopeId, [transactionKey]),
    { key: authorizationKey, value: nextRecord(authorization, { state: 'consumed', transactionId: request.transactionId }) },
    { key: sessionKey, value: nextRecord(session, { state: 'authorization_consumed', lastStateBeforeTerminal: session.state,
      futureActions: 'fenced', fencedAt: now }) },
    { key: preparationKey, value: nextRecord(preparation, { state: 'executing', transactionId: request.transactionId }) },
    { key: transactionKey, value: transaction },
    { key: binding.record.outcomeKey, value: outcomeValue(authorization, request, 'consumed', now) },
    { key: binding.record.timeoutKey, value: { revision: 1, transactionId: request.transactionId, dueAt: transaction.timeoutAt, state: 'pending' } },
  ];
  if (request.purpose === 'activate') {
    reads.push(...waitingStagedReads(records, stagedKey, session, preparation, claims, now));
    const staged = readRecord(records, stagedKey);
    reads.push(stagedKey); writes.push({ key: stagedKey,
      value: nextRecord(staged, { state: 'activating', activationTransactionId: request.transactionId }) });
    writes.find((write) => write.key === preparationKey).value.waitingStagedKey = null;
  } else requireCondition(stagedKey === null, 'MODEL_STAGED_INVALID');
  return commandPlan(records, command, now, reads, writes,
    [{ kind: 'authorization-consumed', transactionId: request.transactionId, committedAt: now }],
    { state: 'consumed', transactionId: request.transactionId });
}

export function planAuthorizationEnd(records, command, now) {
  const { authorization, session } = authorizationRecords(records, command);
  requireCondition(authorization.revision === command.expectedAuthorizationRevision && authorization.state === 'available'
    && session.state === 'authorized' && ['cancel', 'expire'].includes(command.action), 'MODEL_AUTHORIZATION_CONFLICT');
  const state = now >= authorization.expiresAt ? 'expired' : 'cancelled';
  requireCondition(command.action !== 'expire' || state === 'expired', 'MODEL_TIMEOUT_EARLY');
  const ended = authorizationEndWrites(records, command.authorizationKey, authorization, state, now);
  return commandPlan(records, command, now, [command.authorizationKey, command.sessionKey, ...ended.reads],
    [...ended.writes, { key: command.sessionKey, value: nextRecord(session,
      { state: state === 'expired' ? 'authorization_expired' : 'cancelled_before_install', lastStateBeforeTerminal: session.state,
        futureActions: 'fenced', fencedAt: now }) }], [],
    { state, transactionId: null });
}

// Late consume registration resolves an already terminal authorization. It
// creates only its atomic minimal outcome, never transaction or renewed rights.
export function planObserveEndedConsume(records, command, now) {
  const { authorization } = authorizationRecords(records, command);
  requireCondition(['cancelled', 'expired'].includes(authorization.state), 'MODEL_AUTHORIZATION_CONFLICT');
  const binding = reservation(records, authorization);
  requireCondition(binding !== null && now < authorization.expiresAt + 62 * DAY, 'MODEL_CONSUME_BINDING_INVALID');
  const existing = records[binding.record.outcomeKey];
  const expected = outcomeValue(authorization, binding.request, authorization.state, authorization.endedAt);
  const writes = existing === undefined ? [{ key: binding.record.outcomeKey, value: expected }] : [];
  if (existing !== undefined) requireCondition(existing.state === authorization.state
    && equalDigest(existing.statusBindingHash, expected.statusBindingHash), 'MODEL_CONSUME_BINDING_INVALID');
  return commandPlan(records, command, now, [command.authorizationKey, command.sessionKey, binding.key,
    ...(existing === undefined ? [] : [binding.record.outcomeKey])], writes, [], { state: authorization.state, transactionId: null });
}

export function planSessionTimeout(records, { sessionKey }, now) {
  const session = readRecord(records, sessionKey); const state = sessionTimeoutState(session, now);
  requireCondition(state !== session.state, 'MODEL_TIMEOUT_EARLY');
  const reads = [sessionKey]; const writes = [{ key: sessionKey, value: nextRecord(session,
    { state, lastStateBeforeTerminal: session.state, futureActions: 'fenced' }) }];
  if (session.state === 'authorized') {
    const { authorization } = authorizationRecords(records, { sessionKey, authorizationKey: session.authorizationKey });
    requireCondition(authorization.state === 'available', 'MODEL_AUTHORIZATION_CONFLICT');
    const ended = authorizationEndWrites(records, session.authorizationKey, authorization, 'expired', now);
    reads.push(session.authorizationKey, ...ended.reads); writes.push(...ended.writes);
  }
  if (session.activeValidateKey !== null) {
    const validate = readRecord(records, session.activeValidateKey);
    requireCondition(validate.sessionId === session.sessionId && validate.state === 'processing', 'MODEL_VALIDATE_CONFLICT');
    reads.push(session.activeValidateKey); writes.push({ key: session.activeValidateKey,
      value: nextRecord(validate, { state: 'coordination_aborted', ownerEpoch: validate.ownerEpoch + 1 }) });
  }
  const timeout = readRecord(records, session.timeoutKey); reads.push(session.timeoutKey);
  writes.push({ key: session.timeoutKey, value: nextRecord(timeout, { state: 'completed' }) });
  if (session.backupCleanupKey !== null) {
    const cleanup = readRecord(records, session.backupCleanupKey); reads.push(session.backupCleanupKey);
    writes.push({ key: session.backupCleanupKey, value: nextRecord(cleanup, { state: 'due' }) });
  }
  return atomicPlan(records, [...new Set(reads)], writes, [], now);
}
export function planTransactionTimeout(records, { transactionKey, timeoutKey }, now) {
  const transaction = readRecord(records, transactionKey); const state = transactionTimeoutState(transaction, now);
  requireCondition(state !== transaction.state, 'MODEL_TIMEOUT_EARLY'); const timeout = readRecord(records, timeoutKey);
  requireCondition(timeout.transactionId === transaction.transactionId && timeout.dueAt === transaction.startedAt + 30 * DAY, 'MODEL_TIMEOUT_INVALID');
  return atomicPlan(records, [transactionKey, timeoutKey], [{ key: transactionKey,
    value: nextRecord(transaction, { state, lastStateBeforeTerminal: transaction.state, reason: 'reporting_timeout' }) },
  { key: timeoutKey, value: nextRecord(timeout, { state: 'completed' }) }],
  [{ kind: 'reporting-timeout', transactionId: transaction.transactionId, localGateDecision: 'unchanged' }], now);
}
export function consumeStatus(outcome, request, now) {
  try {
    assertApiShape('status-request', request);
    const secret = Buffer.from(request.recoverySecret, 'base64url');
    requireCondition(secret.toString('base64url') === request.recoverySecret, 'UNKNOWN_CONSUME_OUTCOME');
    const { recoverySecret, protocolVersion, ...fields } = request;
    const binding = statusBindingHash({ ...fields, recoveryHandleHash: recoveryHandleHash(secret) });
    requireCondition(outcome !== undefined && consumeQueryAvailable(outcome.authorizationExp, now)
      && equalDigest(outcome.statusBindingHash, binding), 'UNKNOWN_CONSUME_OUTCOME');
    return { state: outcome.state, transactionId: outcome.transactionId };
  } catch { requireCondition(false, 'UNKNOWN_CONSUME_OUTCOME'); }
}
