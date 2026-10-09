import test from 'node:test';
import assert from 'node:assert/strict';
import { applyAtomically } from '../models/atomic.mjs';
import { planConsume, planSessionTimeout } from '../models/lifecycle.mjs';
import { planBeginValidate, planFinishValidate } from '../models/validate.mjs';
import { planCompletePreinstall, planRebuildWaiting } from '../models/staged.mjs';
import { planDownloadHandoff } from '../models/workflow.mjs';
import { objectHash } from '../models/inputs.mjs';
import { envelopeReference } from '../signatures.mjs';
import { consumeFixture, modelTransaction, modelFacts, operation, operationCommand, addQualification } from './model-fixtures.mjs';
import { sessionFixture, emit } from './session-fixtures.mjs';
import { NOW, HASH, createFixtureSet } from './fixtures.mjs';
import { DAY } from '../semantics.mjs';

test('consume serializes the complete scope and allows only historical terminal transactions', () => {
  for (const purpose of ['install', 'preinstall', 'activate']) {
    const { records, command } = consumeFixture(purpose);
    records.other = { ...modelTransaction(), transactionId: 'other-transaction' }; records.index.memberKeys.push('other');
    assert.throws(() => planConsume(records, command, NOW + 1), { code: 'MODEL_SCOPE_OCCUPIED' });
    records.other.state = 'succeeded'; const plan = planConsume(records, command, NOW + 1);
    const raced = structuredClone(records); raced.other.revision++; raced.other.state = 'authorization_consumed';
    assert.throws(() => applyAtomically(raced, plan, NOW + 1), { code: 'MODEL_CAS_CONFLICT' });
    assert.equal(applyAtomically(records, plan, NOW + 1).records.transaction.state, 'authorization_consumed');
  }
});

test('completion and cancelled-activation rebuild cannot create another waiting slot', () => {
  for (const rebuilding of [false, true]) {
    const { records, command, context } = consumeFixture(rebuilding ? 'activate' : 'preinstall');
    const consumed = applyAtomically(records, planConsume(records, command, NOW + 1), NOW + 1).records;
    consumed.transaction.state = rebuilding ? 'cancelled_before_install' : 'staging_verified';
    consumed.operation = operation(NOW + 2); consumed.proof = { revision: 1, ...modelFacts() };
    consumed.channel = { revision: 1, channelRevision: 1, installationScopeId: 'scope-1' };
    if (rebuilding) consumed.staged.immutableUpdate = structuredClone(consumed.transaction.immutableUpdate);
    consumed.other = { revision: 1, recordKind: 'staged', installationScopeId: 'scope-1', state: 'waiting', slotIdentity: 'other-slot' };
    consumed.index.memberKeys.push('other');
    const complete = { ...operationCommand, transactionKey: 'transaction', cancelledTransactionKey: 'transaction',
      slotProofKey: 'proof', safetyProofKey: 'proof', channelKey: 'channel', preparationKey: 'preparation',
      stagedKey: 'staged', newWaitingKey: 'new-waiting', scopeIndexKey: 'index', qualificationKey: 'qualification', context };
    const planner = rebuilding ? planRebuildWaiting : planCompletePreinstall;
    assert.throws(() => planner(consumed, complete, NOW + 2), { code: 'MODEL_SCOPE_OCCUPIED' });
    consumed.other.state = 'invalidated'; const plan = planner(consumed, complete, NOW + 2);
    const raced = structuredClone(consumed); raced.other.revision++; raced.other.state = 'waiting';
    assert.throws(() => applyAtomically(raced, plan, NOW + 2), { code: 'MODEL_CAS_CONFLICT' });
    assert.equal(applyAtomically(consumed, plan, NOW + 2).records[rebuilding ? 'new-waiting' : 'staged'].state, 'waiting');
  }
});

test('activate validate checks exact waiting facts and their original preinstall CAS', () => {
  const { set, records, command, claims, context } = consumeFixture('activate');
  delete records.authorization; delete records.reservation; delete records.operation;
  records.index.memberKeys = records.index.memberKeys.filter((key) => key !== 'authorization');
  Object.assign(records.session, { state: 'preparation_ready', authorizationKey: null });
  const begin = { ...operationCommand, sessionKey: 'session', preparationKey: 'preparation', scopeIndexKey: 'index', validateKey: 'validate' };
  const coordinated = applyAtomically(records, planBeginValidate(records, begin, NOW + 1), NOW + 1).records;
  const finish = { ...operationCommand, sessionKey: 'session', result: 'authorized', authorizationKey: 'authorization',
    rootKey: 'root', scopeIndexKey: 'index', qualificationKey: 'qualification', expectedContext: context, authorizationBytes: command.authorizationBytes };
  const plan = planFinishValidate(coordinated, finish, NOW + 2);
  assert.equal(applyAtomically(coordinated, plan, NOW + 2).records.authorization.state, 'available');
  for (const mutate of [(copy) => { copy.staged.state = 'invalidated'; },
    (copy) => { copy['original-preinstall'].state = 'staging_failed'; },
    (copy) => { copy.preparation.waitingStagedKey = null; }]) {
    const copy = structuredClone(coordinated); mutate(copy);
    assert.throws(() => planFinishValidate(copy, finish, NOW + 2));
    assert.throws(() => applyAtomically(copy, plan, NOW + 2), { code: 'MODEL_CAS_CONFLICT' });
  }
  for (const field of ['slotIdentity', 'preinstallTransactionId', 'stagedAt', 'stagedValidUntil']) {
    const altered = structuredClone(claims); altered.staged[field] = typeof altered.staged[field] === 'string' ? 'other-id' : altered.staged[field] + 1;
    const copy = structuredClone(coordinated); addQualification(copy, context, { claims: altered });
    assert.throws(() => planFinishValidate(copy, { ...finish, authorizationBytes: set.sign(altered) }, NOW + 2), (error) => ['MODEL_STAGED_INVALID', 'STAGED_WINDOW_INVALID'].includes(error.code));
  }
});

test('actual constructed download session hands off after forty days and completes fresh preparation events', () => {
  const fixture = sessionFixture({ required: true }); let records = fixture.records;
  const facts = { ...modelFacts(), confirmedHopSha256: records.session.hopSha256 };
  for (const [index, state] of ['update_offered', 'user_confirmed', 'download_started'].entries()) records = emit(fixture, records, state, facts, NOW + index + 1);
  records = applyAtomically(records, planSessionTimeout(records, { sessionKey: 'session' }, NOW + DAY), NOW + DAY).records;
  const now = NOW + 40 * DAY; const fresh = createFixtureSet({ clock: now, signingKeys: fixture.set.keys });
  const decision = fresh.claims.decision; decision.telemetrySessionId = 'new-session';
  decision.metadataSet.root = envelopeReference(fixture.set.bytes.root);
  decision.update.originalStage = structuredClone(fixture.decision.update.originalStage);
  decision.update.backupPolicy = 'required'; decision.update.plans.backup = structuredClone(fixture.decision.update.plans.backup);
  records.operation = operation(now); records.preparation = { revision: 1, state: 'idle', installationScopeId: 'scope-1',
    ownerEpoch: 2, dataScopeSha256: HASH, freezeEpoch: null, backup: null, transactionId: null };
  const context = { immutableContextSha256: records.session.immutableContextSha256 };
  addQualification(records, context, { owner: 'T17', now, claims: decision }); records.qualification.contentIdentity = records.workflow.immutableContext.contentIdentity;
  const handoff = { ...operationCommand, workflowKey: 'workflow', oldSessionKey: 'session', newSessionKey: 'fresh-session',
    qualificationKey: 'qualification', scopeIndexKey: 'index', rootKey: 'root', preparationKey: 'preparation', expectedRevision: records.workflow.revision,
    timeoutKey: 'fresh-timeout', diagnosticCursorKey: 'fresh-diagnostic', decisionBytes: fixture.set.sign(decision),
    expectedContext: { role: 'decision', product: decision.product, component: decision.component, scope: decision.scope } };
  const plan = planDownloadHandoff(records, handoff, now); records = applyAtomically(records, plan, now).records;
  assert.equal(records['fresh-session'].sequence, 0); assert.equal(records['fresh-session'].freezeEpoch, null);
  assert.equal(records['fresh-session'].groupIdentity, fixture.records.session.groupIdentity);
  const oldSession = records.session;
  // The event helper chooses the newly constructed lineage and new credential.
  fresh.claims['telemetry-event'].telemetrySessionId = 'new-session'; fresh.claims['telemetry-event'].allowedEvents = ['update_offered'];
  const eventFixture = { ...fixture, set: fresh };
  const currentFacts = { ...facts, ownerEpoch: 2, backupPreparationEpoch: 2, workflowId: 'workflow-1', writableSessionId: 'new-session',
    hopSha256: records['fresh-session'].hopSha256 };
  for (const [index, state] of ['update_offered', 'download_resume_context', 'download_started', 'download_succeeded', 'verification_started',
    'verification_succeeded', 'preflight_started', 'preflight_succeeded', 'permission_checked', 'permission_granted',
    'preparation_started', 'writers_frozen', 'backup_started', 'backup_succeeded', 'preparation_ready'].entries())
    records = emit(eventFixture, records, state, currentFacts, now + index + 1, now + index + 1, 'fresh-session');
  assert.equal(records['fresh-session'].state, 'preparation_ready'); assert.equal(records['fresh-session'].preparationEpoch, 2);
  assert.equal(records['fresh-session'].freezeEpoch, 1); assert.equal(oldSession.state, 'expired');
  assert.equal(records['fresh-session'].observationWindowId, oldSession.observationWindowId);
});
