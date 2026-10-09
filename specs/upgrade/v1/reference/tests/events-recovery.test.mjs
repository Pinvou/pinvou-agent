import test from 'node:test';
import assert from 'node:assert/strict';
import { planEvent } from '../models/events.mjs';
import { planRetireExecution, assertExecutionNotRetired, planReconcileRetired } from '../models/recovery.mjs';
import { applyAtomically, captureRecord } from '../models/atomic.mjs';
import { objectHash } from '../models/inputs.mjs';
import { canonicalize, parseJson } from '../canonical-json.mjs';
import { modelTransaction, modelFacts, operation, operationCommand } from './model-fixtures.mjs';
import { NOW, HASH, createFixtureSet, TARGET } from './fixtures.mjs';
import { DAY } from '../semantics.mjs';
import { approvalContext } from '../models/entities.mjs';

function eventFixture(purpose = 'install') {
  const set = createFixtureSet(); const claims = set.claims[`${purpose}-transaction-event`];
  claims.allowedEvents = ['execution_ready', 'staging_started', 'activation_ready'];
  const facts = { factType: 'local-state' };
  const toState = purpose === 'install' ? 'execution_ready' : purpose === 'preinstall' ? 'staging_started' : 'activation_ready';
  const event = { eventId: 'event-1', sequence: 1, occurredAt: NOW + 1, lineageId: 'transaction-1', executionPurpose: purpose,
    kind: toState, fromState: 'authorization_consumed', toState, facts, factsSha256: objectHash(facts), targetIdentitySha256: HASH };
  const records = { root: { revision: 1, published: true, body: set.metadata.root }, deny: { revision: 1, recordKind: 'deny', entries: [] },
    transaction: modelTransaction('authorization_consumed', purpose), 'contribution-head': { revision: 1, watermark: 0, windowGroups: {} } };
  const command = { rootKey: 'root', denyKey: 'deny', lineageKey: 'transaction', ledgerKey: 'ledger', outboxKey: 'outbox',
    credential: parseJson(set.sign(claims)), expectedContext: { role: claims.role, product: claims.product, component: claims.component, scope: claims.scope },
    scope: { product: 'pinvou', component: 'app', installId: 'install-1', installationScopeId: 'scope-1', channel: 'stable', channelRevision: 1, targetKey: TARGET }, event };
  return { set, records, command, claims };
}
test('event state/ledger/outbox/contribution share the commit; replay and deny order cannot leak existence', () => {
  const { records, command, claims } = eventFixture();
  const plan = planEvent(records, command, NOW + 2); const result = applyAtomically(records, plan, NOW + 2).records;
  assert.equal(result.transaction.state, 'execution_ready'); assert.equal(result.ledger.retainUntil, claims.exp + 120_000);
  assert.equal(result['contribution-head'].watermark, 1); assert.equal(result.outbox.contributionWatermark, 1);
  const replay = applyAtomically(result, planEvent(result, command, NOW + 3), NOW + 3).records;
  assert.equal(replay.transaction.revision, result.transaction.revision); assert.equal(replay['contribution-head'].watermark, 1);
  for (const exists of [false, true]) {
    const denied = structuredClone(exists ? result : records);
    denied.deny.entries.push({ subjectKind: 'signingKey', subjectId: claims.signingKeyId, roles: [claims.role] });
    assert.throws(() => planEvent(denied, command, NOW + 3), { code: 'EVENT_KEY_DENIED' });
  }
  const changed = structuredClone(records); changed.deny.revision += 1;
  assert.throws(() => applyAtomically(changed, plan, NOW + 2), { code: 'MODEL_CAS_CONFLICT' });
});
test('sequence rollback, illegal edge and changed digest cannot commit a second event', () => {
  const { records, command } = eventFixture();
  assert.throws(() => planEvent(records, { ...command, event: { ...command.event, sequence: 2 } }, NOW + 2));
  assert.throws(() => planEvent(records, { ...command, event: { ...command.event, fromState: 'health_check_started' } }, NOW + 2));
  const result = applyAtomically(records, planEvent(records, command, NOW + 2), NOW + 2).records;
  assert.throws(() => planEvent(result, { ...command, event: { ...command.event, sequence: 2 } }, NOW + 3), { code: 'IDEMPOTENCY_CONFLICT' });
});
test('37-day lawful late event is diagnostic and cannot revive a 30-day terminal', () => {
  const { records, command } = eventFixture(); records.transaction.state = 'failed_manual_repair_required';
  records.transaction.lastStateBeforeTerminal = 'authorization_consumed';
  const late = applyAtomically(records, planEvent(records, command, NOW + 30 * DAY), NOW + 30 * DAY).records;
  assert.equal(late.transaction.state, 'failed_manual_repair_required'); assert.equal(late.ledger.disposition, 'late-diagnostic');
  assert.throws(() => planEvent(records, command, NOW + 37 * DAY));
  assert.throws(() => planEvent(records, { ...command, event: { ...command.event, toState: 'succeeded' } }, NOW + 30 * DAY));
});
test('unknown consume retirement keeps permanent fence and reconciliation occupancy after local freeze release', () => {
  const attempt = { revision: 1, state: 'consume_pending', firstSentAt: NOW, ownerEpoch: 1, authorizationJti: 'jti-1', transactionId: 'transaction-1',
    consumeRequestDigest: HASH, oldIdentitySha256: HASH, oldDataSha256: HASH, purpose: 'install', installationScopeId: 'scope-1',
    product: 'pinvou', component: 'app', channel: 'stable', targetKey: TARGET };
  const records = { attempt, protection: { revision: 1, state: 'preparing', ownerEpoch: 1, installationScopeId: 'scope-1', ownFreezeState: 'held' },
    proof: { revision: 1, ...modelFacts(), installationScopeId: 'scope-1', executionPurpose: 'install', transactionId: 'transaction-1', authorizationJti: 'jti-1' } };
  const command = { attemptKey: 'attempt', protectionKey: 'protection', proofKey: 'proof', fenceKey: 'fence', trigger: 'wait_exhausted' };
  assert.throws(() => planRetireExecution(records, command, NOW + 299_999));
  for (const trigger of ['user_stopped', 'trusted_budget_unavailable']) assert.doesNotThrow(() =>
    planRetireExecution(records, { ...command, trigger }, NOW + 1));
  const wrongScope = structuredClone(records); wrongScope.protection.installationScopeId = 'scope-2';
  assert.throws(() => planRetireExecution(wrongScope, command, NOW + 300_000));
  const retired = applyAtomically(records, planRetireExecution(records, command, NOW + 300_000), NOW + 300_000).records;
  assert.equal(retired.protection.state, 'reconciliation_required'); assert.equal(retired.protection.ownFreezeState, 'released');
  assert.throws(() => assertExecutionNotRetired(retired.fence, { authorizationJti: 'jti-1', transactionId: 'transaction-1', ownerEpoch: 2 }));
  assertExecutionNotRetired(retired.fence, { authorizationJti: 'new-jti', transactionId: 'new-transaction', ownerEpoch: 2 });
  retired.task = { revision: 1, taskId: 'task-1', state: 'observing', taskPurpose: 'reconcile', windowStart: NOW, windowEnd: NOW + DAY,
    historicalOutcome: 'unknown', binding: { originalTransactionId: 'transaction-1', originalAuthorizationJti: 'jti-1',
      originalConsumeRequestDigest: HASH, originalPurpose: 'install', installationScopeId: 'scope-1' } };
  retired.approval = { revision: 1, projectionOwner: 'T05', state: 'approved', bodySha256: objectHash(retired.task.binding),
    authorId: 'author', reviewerIds: ['reviewer-a', 'reviewer-b'], context: approvalContext('ReconcileRetiredExecution', attempt, 'task-1', 1) };
  retired.operation = operation(NOW + 300_001);
  retired.serverOccupancy = { revision: 1, state: 'reconciliation_required', installationScopeId: 'scope-1',
    originalAuthorizationJti: 'jti-1', originalTransactionId: 'transaction-1' };
  retired.historyIndex = { revision: 1, installationScopeId: 'scope-1', byAuthorizationJti: {} };
  retired.serverDisposition = { revision: 1, projectionOwner: 'T22', state: 'reviewed', taskId: 'task-1',
    originalAuthorizationJti: 'jti-1', originalTransactionId: 'transaction-1', originalConsumeRequestDigest: HASH,
    installationScopeId: 'scope-1', originalPurpose: 'install', historicalOutcome: 'unknown', authorizationKey: null,
    historyIndexKey: 'historyIndex', occupancyKey: 'serverOccupancy', readSet: [captureRecord(retired, 'serverOccupancy'), captureRecord(retired, 'historyIndex')] };
  const reconcile = { ...operationCommand, taskKey: 'task', attemptKey: 'attempt', proofKey: 'proof', protectionKey: 'protection',
    approvalKey: 'approval', dispositionKey: 'disposition', serverDispositionKey: 'serverDisposition' };
  const safe = applyAtomically(retired, planReconcileRetired(retired, reconcile, NOW + 300_001), NOW + 300_001).records;
  assert.equal(safe.protection.state, 'idle'); assert.equal(safe.disposition.historicalOutcome, 'unknown');
  assert.equal(safe.attempt.state, 'local_execution_retired'); assert.equal(safe.fence.state, 'permanent');
});
