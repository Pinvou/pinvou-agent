import test from 'node:test';
import assert from 'node:assert/strict';
import { planConsume, planAuthorizationEnd, planSessionTimeout, planTransactionTimeout, consumeStatus, planObserveEndedConsume } from '../models/lifecycle.mjs';
import { applyAtomically } from '../models/atomic.mjs';
import { assertTransactionTransition } from '../models/transaction.mjs';
import { planChannelChange } from '../models/channel.mjs';
import { operation, operationCommand, consumeFixture, modelFacts } from './model-fixtures.mjs';
import { NOW } from './fixtures.mjs';
import { DAY } from '../semantics.mjs';
import { objectHash } from '../models/inputs.mjs';

test('each purpose consumes with outcome, timeout, ownership and complete transaction bindings', () => {
  for (const purpose of ['preinstall', 'install', 'activate']) {
    const { records, command } = consumeFixture(purpose); const before = structuredClone(records); const originalHash = objectHash(records);
    const plan = planConsume(records, command, NOW + 1); const result = applyAtomically(records, plan, NOW + 1).records;
    assert.equal(objectHash(records), originalHash);
    assert.equal(result.authorization.state, 'consumed'); assert.equal(result.session.state, 'authorization_consumed');
    assert.equal(result.outcome.state, 'consumed'); assert.equal(result.outcome.transactionId, 'transaction-1');
    assert.equal(result.preparation.state, 'executing'); assert.equal(result.preparation.ownerEpoch, before.preparation.ownerEpoch);
    assert.equal(result['transaction-timeout'].dueAt, NOW + 1 + 30 * DAY);
    assert.equal(result.operation.state, 'committed'); assert(result.index.memberKeys.includes('transaction'));
    const edge = purpose === 'install' ? 'execution_ready' : purpose === 'activate' ? 'activation_ready' : 'staging_started';
    assert.doesNotThrow(() => assertTransactionTransition(result.transaction, edge, { now: NOW + 2, facts: modelFacts() }));
    if (purpose === 'activate') assert.equal(result.staged.state, 'activating');
  }
});
test('every captured consume input change or commit-time change yields zero writes', () => {
  const { records, command } = consumeFixture(); const plan = planConsume(records, command, NOW + 1);
  for (const read of plan.readSet) {
    const changed = structuredClone(records); changed[read.key].revision += 1; const before = structuredClone(changed);
    assert.throws(() => applyAtomically(changed, plan, NOW + 1), { code: 'MODEL_CAS_CONFLICT' }); assert.deepEqual(changed, before);
  }
  assert.throws(() => applyAtomically(records, plan, NOW + 2), { code: 'MODEL_FINAL_GUARDS_REQUIRED' });
  plan.writes[0].value.state = 'available'; assert.throws(() => applyAtomically(records, plan, NOW + 1), { code: 'MODEL_PLAN_CHANGED' });
});

test('every first-consume purpose requires the actual writable workflow and its exact immutable intent', () => {
  for (const purpose of ['preinstall', 'install', 'activate']) {
    const { records, command } = consumeFixture(purpose);
    for (const mutate of [(copy) => { copy.workflow.state = 'failed'; },
      (copy) => { copy.workflow.writableSessionId = 'other-session'; },
      (copy) => { copy.workflow.workflowId = 'other-workflow'; },
      (copy) => { copy.workflow.immutableContext.hopId = 'other-hop'; },
      (copy) => { copy.session.immutableContextSha256 = 'b'.repeat(64); }]) {
      const changed = structuredClone(records); mutate(changed); const before = objectHash(changed);
      assert.throws(() => planConsume(changed, command, NOW + 1), { code: 'MODEL_WORKFLOW_ENDED' });
      assert.equal(objectHash(changed), before);
    }
    if (purpose === 'install') {
      const changed = structuredClone(records); changed.workflow.confirmedHopSha256 = null;
      assert.throws(() => planConsume(changed, command, NOW + 1), { code: 'MODEL_WORKFLOW_ENDED' });
    }
  }
});
test('consume and cancel race in both orders; only one terminal outcome and at most one transaction', () => {
  for (const first of ['consume', 'cancel']) {
    const { records, command } = consumeFixture(); records.cancelOperation = operation();
    const consume = planConsume(records, command, NOW + 1);
    const cancel = planAuthorizationEnd(records, { ...command, operationKey: 'cancelOperation', action: 'cancel' }, NOW + 1);
    const winning = applyAtomically(records, first === 'consume' ? consume : cancel, NOW + 1).records;
    assert.throws(() => applyAtomically(winning, first === 'consume' ? cancel : consume, NOW + 1), { code: 'MODEL_CAS_CONFLICT' });
    assert.equal(winning.outcome.state, first === 'consume' ? 'consumed' : 'cancelled');
    assert.equal(Object.hasOwn(winning, 'transaction'), first === 'consume');
  }
});
test('at/after authorization expiry cancel records expired and consume never creates rights', () => {
  for (const offset of [0, 1]) {
    const { records, command, claims } = consumeFixture(); const now = claims.exp + offset;
    records.operation = operation(now);
    assert.throws(() => planConsume(records, command, now), { code: 'MODEL_AUTHORIZATION_EXPIRED' });
    const result = applyAtomically(records, planAuthorizationEnd(records, { ...command, action: 'cancel' }, now), now).records;
    assert.equal(result.outcome.state, 'expired'); assert.equal(result.outcome.transactionId, null);
    assert.equal(result.session.state, 'authorization_expired'); assert(!Object.hasOwn(result, 'transaction'));
  }
});
test('registered purpose/scope/request alterations cannot forge cancellation outcomes', () => {
  for (const [field, value] of [['purpose', 'preinstall'], ['installationScopeId', 'other-scope'], ['transactionId', 'other-transaction']]) {
    const { records, command } = consumeFixture(); records.reservation.request[field] = value;
    assert.throws(() => planAuthorizationEnd(records, { ...command, action: 'cancel' }, NOW + 1));
  }
});
test('session timeout derives all mandatory associations rather than optional caller keys', () => {
  const { records } = consumeFixture();
  const expired = applyAtomically(records, planSessionTimeout(records, { sessionKey: 'session' }, NOW + DAY), NOW + DAY).records;
  assert.equal(expired.session.state, 'authorization_expired'); assert.equal(expired.authorization.state, 'expired');
  assert.equal(expired.outcome.state, 'expired'); assert.equal(expired['session-timeout'].state, 'completed');
  const missing = structuredClone(records); delete missing.authorization;
  assert.throws(() => planSessionTimeout(missing, { sessionKey: 'session' }, NOW + DAY));
});
test('consume-status binds every field, closes at +61d and never returns credentials', () => {
  const { records, command, claims } = consumeFixture();
  const result = applyAtomically(records, planConsume(records, command, NOW + 1), NOW + 1).records;
  const { product, component, installationScopeId, purpose, authorizationJti, consumeKey, transactionId, consumeRequestDigest } = command.request;
  const request = { protocolVersion: 1, product, component, installationScopeId, purpose, authorizationJti, consumeKey,
    transactionId, consumeRequestDigest, recoverySecret: Buffer.alloc(32, 1).toString('base64url') };
  assert.deepEqual(consumeStatus(result.outcome, request, claims.exp + 61 * DAY - 1), { state: 'consumed', transactionId });
  for (const offset of [0, 1]) assert.throws(() => consumeStatus(result.outcome, request, claims.exp + 61 * DAY + offset), { code: 'UNKNOWN_CONSUME_OUTCOME' });
  for (const field of ['product', 'component', 'installationScopeId', 'purpose', 'authorizationJti', 'consumeKey', 'transactionId', 'consumeRequestDigest', 'recoverySecret']) {
    const wrong = { ...request, [field]: field === 'purpose' ? 'activate' : field === 'consumeRequestDigest'
      ? 'b'.repeat(64) : field === 'recoverySecret' ? Buffer.alloc(32, 2).toString('base64url') : 'wrong' };
    assert.throws(() => consumeStatus(result.outcome, wrong, NOW + 2), { code: 'UNKNOWN_CONSUME_OUTCOME' });
  }
  assert.throws(() => consumeStatus(undefined, request, NOW + 2), { code: 'UNKNOWN_CONSUME_OUTCOME' });
});
test('ended authorization with a later reservation records only the original terminal outcome', () => {
  const { records, command } = consumeFixture(); records.authorization.consumeReservationKey = null;
  let ended = applyAtomically(records, planAuthorizationEnd(records, { ...command, action: 'cancel' }, NOW + 1), NOW + 1).records;
  assert(!Object.hasOwn(ended, 'outcome'));
  ended.authorization.consumeReservationKey = 'reservation'; ended.authorization.revision += 1;
  ended.operation = operation(NOW + 2);
  ended = applyAtomically(ended, planObserveEndedConsume(ended, command, NOW + 2), NOW + 2).records;
  assert.equal(ended.outcome.state, 'cancelled'); assert.equal(ended.outcome.committedAt, NOW + 1);
  assert(!Object.hasOwn(ended, 'transaction'));
});
test('channel switch and consumption compete on the same scope; consumed install continues', () => {
  const { records, command } = consumeFixture(); records.channel = { revision: 1, channel: 'stable', channelRevision: 1, installationScopeId: 'scope-1' };
  records.changeOperation = operation();
  const change = { ...operationCommand, operationKey: 'changeOperation', channelKey: 'channel', scopeIndexKey: 'index',
    expectedChannelRevision: 1, newChannel: 'beta' };
  const plan = planConsume(records, command, NOW + 1); const switchPlan = planChannelChange(records, change, NOW + 1);
  const switched = applyAtomically(records, switchPlan, NOW + 1).records;
  assert.equal(switched.authorization.state, 'cancelled'); assert.equal(switched.outcome.state, 'cancelled');
  assert.equal(switched.session.state, 'channel_changed'); assert.throws(() => applyAtomically(switched, plan, NOW + 1));
  const consumed = applyAtomically(records, plan, NOW + 1).records;
  const changed = applyAtomically(consumed, planChannelChange(consumed, change, NOW + 2), NOW + 2).records;
  assert.equal(changed.transaction.state, 'authorization_consumed'); assert.equal(changed.authorization.state, 'consumed');
});
test('30-day missing report closes only server state and leaves local gate decision unchanged', () => {
  const { records, command } = consumeFixture('preinstall');
  const consumed = applyAtomically(records, planConsume(records, command, NOW + 1), NOW + 1).records;
  const now = NOW + 1 + 30 * DAY;
  const result = applyAtomically(consumed, planTransactionTimeout(consumed, { transactionKey: 'transaction', timeoutKey: 'transaction-timeout' }, now), now);
  assert.equal(result.records.transaction.state, 'staging_failed'); assert.equal(result.facts[0].localGateDecision, 'unchanged');
});
