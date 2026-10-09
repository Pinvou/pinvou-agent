import test from 'node:test';
import assert from 'node:assert/strict';
import { planBeginValidate, planFinishValidate } from '../models/validate.mjs';
import { planCompletePreinstall, planRebuildWaiting } from '../models/staged.mjs';
import { planConsume, planSessionTimeout } from '../models/lifecycle.mjs';
import { applyAtomically } from '../models/atomic.mjs';
import { operation, operationCommand, consumeFixture, modelFacts, addQualification } from './model-fixtures.mjs';
import { planAcquireOwner } from '../models/operation.mjs';
import { NOW, HASH } from './fixtures.mjs';
import { DAY } from '../semantics.mjs';

test('one active validate; timeout fences a prepared completion and cannot leave a new authorization', () => {
  const { records, command, context } = consumeFixture();
  delete records.authorization; delete records.reservation; records.index.memberKeys = ['session'];
  delete records.operation;
  Object.assign(records.session, { state: 'preparation_ready', authorizationKey: null });
  const begin = { ...operationCommand, sessionKey: 'session', preparationKey: 'preparation', scopeIndexKey: 'index', validateKey: 'validate' };
  const plan = planBeginValidate(records, begin, NOW + 1);
  let coordinated = applyAtomically(records, plan, NOW + 1).records;
  assert.equal(coordinated.session.state, 'authorization_requested'); assert.equal(coordinated.operation.state, 'processing');
  assert.throws(() => applyAtomically(coordinated, plan, NOW + 1));
  const finish = { ...operationCommand, sessionKey: 'session', result: 'authorized', authorizationKey: 'authorization',
    rootKey: 'root', scopeIndexKey: 'index', qualificationKey: 'qualification', expectedContext: context, authorizationBytes: command.authorizationBytes };
  const prepared = planFinishValidate(coordinated, finish, NOW + 2);
  const result = applyAtomically(coordinated, prepared, NOW + 2).records;
  assert.equal(result.session.state, 'authorized'); assert.equal(result.authorization.state, 'available');
  assert.equal(result.session.activeValidateKey, null); assert.equal(result.validate.state, 'authorized');
  const expired = applyAtomically(coordinated, planSessionTimeout(coordinated, { sessionKey: 'session' }, NOW + DAY), NOW + DAY).records;
  assert.equal(expired.validate.state, 'coordination_aborted'); assert.equal(expired.validate.ownerEpoch, 2);
  assert.throws(() => applyAtomically(expired, prepared, NOW + 2), { code: 'MODEL_CAS_CONFLICT' });
  assert(!Object.hasOwn(expired, 'authorization'));
});
test('validate first registration, same-request recovery and owner takeover share one lineage and reject target rewrites', () => {
  const { set, records, command, claims, context } = consumeFixture();
  delete records.authorization; delete records.reservation; delete records.operation; records.index.memberKeys = ['session'];
  Object.assign(records.session, { state: 'preparation_ready', authorizationKey: null });
  const begin = { ...operationCommand, sessionKey: 'session', preparationKey: 'preparation', scopeIndexKey: 'index', validateKey: 'validate' };
  const prepared = planBeginValidate(records, begin, NOW + 1);
  const coordinated = applyAtomically(records, prepared, NOW + 1).records;
  assert.equal(planBeginValidate(coordinated, begin, NOW + 2).writes.length, 0);
  assert.throws(() => planBeginValidate(coordinated, { ...begin, validateKey: 'competing' }, NOW + 2), { code: 'MODEL_VALIDATE_CONFLICT' });
  assert.throws(() => planBeginValidate(coordinated, { ...begin, requestDigest: 'b'.repeat(64) }, NOW + 2));
  const takeover = applyAtomically(coordinated, planAcquireOwner(coordinated,
    { operationKey: 'operation', expectedRevision: 1 }, NOW + 30_001), NOW + 30_001).records;
  const recovered = applyAtomically(takeover, planBeginValidate(takeover, { ...begin, ownerEpoch: 2 }, NOW + 30_002), NOW + 30_002).records;
  assert.equal(recovered.validate.ownerEpoch, 2); assert.equal(recovered.session.activeValidateKey, 'validate');
  const finish = { ...operationCommand, sessionKey: 'session', result: 'authorized', authorizationKey: 'authorization', rootKey: 'root',
    scopeIndexKey: 'index', qualificationKey: 'qualification', expectedContext: context, authorizationBytes: command.authorizationBytes };
  for (const change of [(body) => { body.update.plans.installation.planId = 'other-approved-install'; },
    (body) => { body.update.plans.takeover.revision++; }, (body) => { body.update.helper.sha256 = 'b'.repeat(64); },
    (body) => { body.sourceProfileId = 'b'.repeat(64); }, (body) => { body.decisionId = 'another-decision'; }]) {
    const altered = structuredClone(claims); change(altered);
    const snapshot = structuredClone(coordinated); addQualification(snapshot, context, { claims: altered });
    assert.throws(() => planFinishValidate(snapshot, { ...finish, authorizationBytes: set.sign(altered) }, NOW + 2), { code: 'MODEL_CONSUME_BINDING_INVALID' });
  }
});
test('preinstall completion atomically releases ownership and creates the original-expiry waiting record', () => {
  const { records, command, context } = consumeFixture('preinstall');
  let consumed = applyAtomically(records, planConsume(records, command, NOW + 1), NOW + 1).records;
  consumed.transaction.state = 'staging_verified'; consumed.operation = operation(NOW + 2);
  consumed.slotProof = { revision: 1, ...modelFacts() }; consumed.channel = { revision: 1, channelRevision: 1, installationScopeId: 'scope-1' };
  const complete = { ...operationCommand, transactionKey: 'transaction', slotProofKey: 'slotProof', channelKey: 'channel',
    preparationKey: 'preparation', stagedKey: 'staged', scopeIndexKey: 'index', qualificationKey: 'qualification', context };
  consumed = applyAtomically(consumed, planCompletePreinstall(consumed, complete, NOW + 2), NOW + 2).records;
  assert.equal(consumed.transaction.state, 'staging_completed'); assert.equal(consumed.preparation.state, 'idle');
  assert.equal(consumed.staged.stagedAt, NOW + 2); assert.equal(consumed.staged.stagedValidUntil, NOW + DAY);
  assert.equal(consumed.staged.preinstallTransactionId, 'transaction-1');
  assert(consumed.index.memberKeys.includes('staged'));
});
test('only confirmed activation safe cancellation can create a higher waiting revision without refreshing its expiry', () => {
  const { records, command, context } = consumeFixture('activate');
  const consumed = applyAtomically(records, planConsume(records, command, NOW + 1), NOW + 1).records;
  consumed.transaction.state = 'cancelled_before_install'; consumed.transaction.immutableUpdate = consumed.authorization.claims.update;
  consumed.staged.immutableUpdate = structuredClone(consumed.transaction.immutableUpdate);
  consumed.operation = operation(NOW + 2); consumed.safety = { revision: 1, ...modelFacts() };
  consumed.channel = { revision: 1, channelRevision: 1, installationScopeId: 'scope-1' };
  const rebuild = { ...operationCommand, stagedKey: 'staged', cancelledTransactionKey: 'transaction', safetyProofKey: 'safety',
    channelKey: 'channel', preparationKey: 'preparation', qualificationKey: 'qualification', context,
    scopeIndexKey: 'index', newWaitingKey: 'waiting-2' };
  const result = applyAtomically(consumed, planRebuildWaiting(consumed, rebuild, NOW + 2), NOW + 2).records;
  assert.equal(result.staged.state, 'cancelled'); assert.equal(result['waiting-2'].state, 'waiting');
  assert(result['waiting-2'].revision > consumed.staged.revision);
  assert.equal(result['waiting-2'].stagedAt, consumed.staged.stagedAt);
  assert.equal(result['waiting-2'].stagedValidUntil, consumed.staged.stagedValidUntil);
  const takeover = structuredClone(consumed); takeover.preparation.ownerEpoch++;
  assert.throws(() => planRebuildWaiting(takeover, rebuild, NOW + 2), { code: 'MODEL_STAGED_INVALID' });
  for (const invalid of ['authorization_consumed', 'failed_manual_repair_required', 'succeeded']) {
    const changed = structuredClone(consumed); changed.transaction.state = invalid;
    assert.throws(() => planRebuildWaiting(changed, rebuild, NOW + 2));
  }
});
