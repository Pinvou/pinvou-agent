import test from 'node:test';
import assert from 'node:assert/strict';
import { planDownloadHandoff } from '../models/workflow.mjs';
import { planEvent } from '../models/events.mjs';
import { applyAtomically } from '../models/atomic.mjs';
import { objectHash } from '../models/inputs.mjs';
import { envelopeReference } from '../signatures.mjs';
import { parseJson } from '../canonical-json.mjs';
import { sessionFixture, emit } from './session-fixtures.mjs';
import { addQualification, modelFacts, operation, operationCommand } from './model-fixtures.mjs';
import { createFixtureSet, NOW, HASH } from './fixtures.mjs';
import { planBeginValidate, planFinishValidate } from '../models/validate.mjs';
import { planConsume, planAuthorizationEnd } from '../models/lifecycle.mjs';
import { sha256, consumeRequestDigest } from '../digests.mjs';
import { assertTransactionTransition } from '../models/transaction.mjs';

function downloadingFixture() {
  const fixture = sessionFixture(); let records = fixture.records;
  const facts = { ...modelFacts(), confirmedHopSha256: records.session.hopSha256 };
  for (const [index, state] of ['update_offered', 'user_confirmed', 'download_started'].entries())
    records = emit(fixture, records, state, facts, NOW + index + 1);
  return { fixture, records, facts };
}
function prepareHandoff(fixture, records, now, fromKey, toKey) {
  const fresh = createFixtureSet({ clock: now, signingKeys: fixture.set.keys }); const decision = fresh.claims.decision;
  decision.telemetrySessionId = toKey; decision.metadataSet.root = envelopeReference(fixture.set.bytes.root);
  decision.update.originalStage = structuredClone(fixture.decision.update.originalStage);
  records.operation = operation(now); records.preparation ??= { revision: 1, state: 'idle', installationScopeId: 'scope-1',
    ownerEpoch: 1, dataScopeSha256: HASH, freezeEpoch: null, backup: null, transactionId: null };
  const context = { immutableContextSha256: records[fromKey].immutableContextSha256 };
  addQualification(records, context, { owner: 'T17', now, claims: decision });
  records.qualification.contentIdentity = records.workflow.immutableContext.contentIdentity;
  const command = { ...operationCommand, workflowKey: 'workflow', oldSessionKey: fromKey, newSessionKey: toKey,
    qualificationKey: 'qualification', scopeIndexKey: 'index', rootKey: 'root', preparationKey: 'preparation',
    expectedRevision: records.workflow.revision, timeoutKey: `${toKey}-timeout`, diagnosticCursorKey: `${toKey}-diagnostic`,
    decisionBytes: fresh.sign(decision), expectedContext: { role: 'decision', product: decision.product, component: decision.component, scope: decision.scope } };
  fresh.claims['telemetry-event'].telemetrySessionId = toKey;
  return { command, fixture: { ...fixture, set: fresh, decision } };
}
function eventCommand(fixture, records, toState, facts, occurredAt, key) {
  const session = records[key]; const claims = fixture.set.claims['telemetry-event']; claims.allowedEvents = [toState];
  const event = { eventId: `${key}-boundary-event`, sequence: session.sequence + 1, lineageId: session.sessionId,
    executionPurpose: session.executionPurpose, occurredAt, kind: toState, fromState: session.state, toState,
    facts, factsSha256: objectHash(facts), targetIdentitySha256: session.finalInstallerSha256 };
  return { rootKey: 'root', denyKey: 'deny', lineageKey: key, ledgerKey: `${event.eventId}-ledger`, outboxKey: `${event.eventId}-outbox`,
    credential: parseJson(fixture.set.sign(claims)), expectedContext: { role: claims.role, product: claims.product, component: claims.component, scope: claims.scope },
    scope: fixture.scope, event };
}

test('an ordinary newly constructed session cannot fabricate a resume edge or original user intent', () => {
  const fixture = sessionFixture(); const records = emit(fixture, fixture.records, 'update_offered', modelFacts(), NOW + 1);
  const facts = { ...modelFacts(), hopSha256: records.session.hopSha256, confirmedHopSha256: records.session.hopSha256 };
  assert.equal(records.workflow.confirmedHopSha256, null);
  assert.throws(() => emit(fixture, records, 'download_resume_context', facts, NOW + 2), { code: 'MODEL_RESUME_INVALID' });
  const forged = structuredClone(records); forged.session.downloadResumeBinding = { workflowId: 'workflow-1',
    fromSessionId: 'previous', toSessionId: 'session-1', hopSha256: forged.session.hopSha256,
    immutableContextSha256: forged.session.immutableContextSha256, confirmedHopSha256: forged.session.hopSha256, handedOffAt: NOW };
  assert.throws(() => emit(fixture, forged, 'download_resume_context', facts, NOW + 2), { code: 'MODEL_RESUME_INVALID' });
});

test('actual handoff registers protected original intent; resume compares workflow and its final CAS', () => {
  const { fixture, records, facts } = downloadingFixture(); const handoff = prepareHandoff(fixture, records, NOW + 10, 'session', 'session-B');
  let result = applyAtomically(records, planDownloadHandoff(records, handoff.command, NOW + 10), NOW + 10).records;
  result = emit(handoff.fixture, result, 'update_offered', facts, NOW + 11, NOW + 11, 'session-B');
  const currentFacts = { ...facts, writableSessionId: 'session-B', hopSha256: result['session-B'].hopSha256 };
  const command = eventCommand(handoff.fixture, result, 'download_resume_context', currentFacts, NOW + 12, 'session-B');
  const plan = planEvent(result, command, NOW + 12);
  for (const mutate of [(copy) => { copy.workflow.confirmedHopSha256 = null; },
    (copy) => { copy.workflow.writableSessionId = 'other-session'; },
    (copy) => { copy.workflow.downloadResumeBinding.fromSessionId = 'other-session'; }]) {
    const raced = structuredClone(result); mutate(raced);
    assert.throws(() => planEvent(raced, command, NOW + 12));
    assert.throws(() => applyAtomically(raced, plan, NOW + 12), { code: 'MODEL_CAS_CONFLICT' });
  }
  result = applyAtomically(result, plan, NOW + 12).records;
  result = emit(handoff.fixture, result, 'download_started', currentFacts, NOW + 13, NOW + 13, 'session-B');
  assert.equal(result['session-B'].state, 'download_started');
  assert.equal(result['session-B'].downloadResumeBinding.fromSessionId, records.session.sessionId);
  assert.equal(result.workflow.confirmedHopSha256, records.workflow.confirmedHopSha256);
});

test('accepted historical download failure atomically ends the workflow and fences future session actions and handoffs', () => {
  const { fixture, records, facts } = downloadingFixture(); const handoff = prepareHandoff(fixture, records, NOW + 10, 'session', 'session-B');
  let current = applyAtomically(records, planDownloadHandoff(records, handoff.command, NOW + 10), NOW + 10).records;
  const currentFacts = { ...facts, writableSessionId: 'session-B', hopSha256: current['session-B'].hopSha256 };
  for (const [index, state] of ['update_offered', 'download_resume_context', 'download_started'].entries())
    current = emit(handoff.fixture, current, state, currentFacts, NOW + 11 + index, NOW + 11 + index, 'session-B');
  const failure = eventCommand(fixture, current, 'download_failed', facts, NOW + 4, 'session');
  const failurePlan = planEvent(current, failure, NOW + 20);
  const prospective = prepareHandoff(fixture, current, NOW + 20, 'session-B', 'session-C');
  // Both valid contenders read the same workflow; only the first can commit.
  const handoffPlan = planDownloadHandoff(current, prospective.command, NOW + 20);
  const failed = applyAtomically(current, failurePlan, NOW + 20).records;
  assert.equal(failed.workflow.state, 'failed'); assert.equal(failed.session.state, 'download_started');
  assert.equal(failed['session-B'].state, 'download_started');
  assert.equal(failed[failure.ledgerKey].disposition, 'late-diagnostic');
  assert.equal(failed['session-diagnostic'].lastState, 'download_failed');
  assert.throws(() => applyAtomically(failed, handoffPlan, NOW + 20), { code: 'MODEL_CAS_CONFLICT' });
  assert.throws(() => planDownloadHandoff(failed, prospective.command, NOW + 20));
  assert.throws(() => emit(handoff.fixture, failed, 'download_succeeded', currentFacts, NOW + 21, NOW + 21, 'session-B'),
    { code: 'MODEL_WORKFLOW_ENDED' });
  const handedOff = applyAtomically(current, handoffPlan, NOW + 20).records;
  assert.throws(() => applyAtomically(handedOff, failurePlan, NOW + 20), { code: 'MODEL_CAS_CONFLICT' });
  // Retry the lawful old failure against the new head; it still latches failure.
  assert.equal(applyAtomically(handedOff, planEvent(handedOff, failure, NOW + 20), NOW + 20).records.workflow.state, 'failed');
});

test('denied, expired, wrongly bound and illegal late failure inputs never latch a workflow', () => {
  const { fixture, records, facts } = downloadingFixture();
  const failure = eventCommand(fixture, records, 'download_failed', facts, NOW + 4, 'session');
  for (const mutate of [(copy, command) => { copy.deny.entries.push({ subjectKind: 'signingKey',
    subjectId: command.credential.signed.signingKeyId, roles: ['telemetry-event'] }); },
    (copy, command) => { command.event.sequence++; }, (copy, command) => { command.event.lineageId = 'other-session'; },
    (copy, command) => { command.event.fromState = 'verification_started'; }]) {
    const copy = structuredClone(records); const command = structuredClone(failure); mutate(copy, command);
    assert.throws(() => planEvent(copy, command, NOW + 5)); assert.equal(copy.workflow.state, 'ongoing');
  }
  assert.throws(() => planEvent(records, failure, fixture.set.claims['telemetry-event'].exp));
  assert.equal(records.workflow.state, 'ongoing');
  const result = applyAtomically(records, planEvent(records, failure, NOW + 5), NOW + 5).records;
  assert.equal(result.workflow.state, 'failed'); assert.equal(result.session.state, 'download_failed');
});

test('workflow confirmation cutoff retains already-occurred current-session diagnostics without restoring its state', () => {
  const { fixture, records, facts } = downloadingFixture(); const handoff = prepareHandoff(fixture, records, NOW + 10, 'session', 'session-B');
  let current = applyAtomically(records, planDownloadHandoff(records, handoff.command, NOW + 10), NOW + 10).records;
  const currentFacts = { ...facts, writableSessionId: 'session-B', hopSha256: current['session-B'].hopSha256 };
  for (const [index, state] of ['update_offered', 'download_resume_context', 'download_started'].entries())
    current = emit(handoff.fixture, current, state, currentFacts, NOW + 11 + index, NOW + 11 + index, 'session-B');
  current = emit(fixture, current, 'download_failed', facts, NOW + 4, NOW + 20);
  assert.equal(current.workflow.failedAt, NOW + 4); assert.equal(current.workflow.endedAt, NOW + 20);
  const oldWorkflow = structuredClone(current.workflow); const oldSession = structuredClone(current['session-B']);
  const command = eventCommand(handoff.fixture, current, 'download_failed', currentFacts, NOW + 14, 'session-B');
  const plan = planEvent(current, command, NOW + 21); const result = applyAtomically(current, plan, NOW + 21).records;
  assert.deepEqual(result.workflow, oldWorkflow); assert.deepEqual(result['session-B'], oldSession);
  assert.equal(result['session-B-diagnostic'].lastState, 'download_failed');
  assert.equal(result[command.ledgerKey].disposition, 'late-diagnostic');
  const raced = structuredClone(current); raced.workflow.revision++;
  assert.throws(() => applyAtomically(raced, plan, NOW + 21), { code: 'MODEL_CAS_CONFLICT' });
  for (const occurredAt of [NOW + 20, NOW + 21]) {
    const future = eventCommand(handoff.fixture, current, 'download_succeeded', currentFacts, occurredAt, 'session-B');
    assert.throws(() => planEvent(current, future, NOW + 21), { code: 'MODEL_WORKFLOW_ENDED' });
  }
});

function preparedHandoffFixture() {
  const { fixture, records, facts } = downloadingFixture(); const handoff = prepareHandoff(fixture, records, NOW + 10, 'session', 'session-B');
  let current = applyAtomically(records, planDownloadHandoff(records, handoff.command, NOW + 10), NOW + 10).records;
  const currentFacts = { ...facts, writableSessionId: 'session-B', hopSha256: current['session-B'].hopSha256 };
  const route = ['update_offered', 'download_resume_context', 'download_started', 'download_succeeded', 'verification_started',
    'verification_succeeded', 'preflight_started', 'preflight_succeeded', 'permission_checked', 'permission_granted',
    'preparation_started', 'writers_frozen', 'preparation_ready'];
  for (const [index, state] of route.entries())
    current = emit(handoff.fixture, current, state, currentFacts, NOW + 11 + index, NOW + 11 + index, 'session-B');
  // Protected T28 owner facts corresponding to the actual acknowledged freeze;
  // these reference tests do not pretend to coordinate real OS writers.
  Object.assign(current.preparation, { state: 'preparing', scopeRevision: 1, freezeEpoch: 1,
    oldIdentitySha256: HASH, oldDataSha256: HASH });
  const claims = structuredClone(handoff.fixture.set.claims['authorization-install']);
  claims.telemetrySessionId = 'session-B'; claims.update = structuredClone(handoff.fixture.decision.update);
  const authorizationBytes = handoff.fixture.set.sign(claims);
  const expectedContext = { role: claims.role, product: claims.product, component: claims.component, scope: claims.scope };
  addQualification(current, expectedContext, { owner: 'T19', now: NOW + 25, claims });
  const begin = { ...operationCommand, operationKey: 'validation-operation', sessionKey: 'session-B',
    preparationKey: 'preparation', scopeIndexKey: 'index', validateKey: 'B-validate' };
  const finish = { ...begin, result: 'authorized', authorizationKey: 'B-authorization', rootKey: 'root',
    qualificationKey: 'qualification', expectedContext, authorizationBytes };
  const request = { ...structuredClone(handoff.fixture.set.consume), authorizationEnvelopeSha256: sha256(authorizationBytes) };
  request.consumeRequestDigest = consumeRequestDigest(request);
  const consume = { ...operationCommand, operationKey: 'consume-operation', sessionKey: 'session-B', authorizationKey: 'B-authorization',
    rootKey: 'root', preparationKey: 'preparation', qualificationKey: 'qualification', scopeIndexKey: 'index',
    transactionKey: 'B-transaction', stagedKey: null, expectedAuthorizationRevision: 1, request, authorizationBytes };
  const failure = (snapshot, now) => emit(fixture, snapshot, 'download_failed', facts, NOW + 4, now);
  return { current, begin, finish, consume, failure };
}

test('confirmed failure before Begin or between Begin/Finish blocks new authorization and all prepared CAS plans', () => {
  const { current, begin, finish, failure } = preparedHandoffFixture();
  const beginPlan = planBeginValidate(current, begin, NOW + 30); const failedBefore = failure(current, NOW + 30);
  assert.throws(() => planBeginValidate(failedBefore, begin, NOW + 30), { code: 'MODEL_WORKFLOW_ENDED' });
  assert.throws(() => applyAtomically(failedBefore, beginPlan, NOW + 30), { code: 'MODEL_CAS_CONFLICT' });
  const coordinated = applyAtomically(current, beginPlan, NOW + 30).records;
  const finishPlan = planFinishValidate(coordinated, finish, NOW + 31); const failedDuring = failure(coordinated, NOW + 31);
  assert.throws(() => planBeginValidate(failedDuring, begin, NOW + 31), { code: 'MODEL_WORKFLOW_ENDED' });
  assert.throws(() => planFinishValidate(failedDuring, finish, NOW + 31), { code: 'MODEL_WORKFLOW_ENDED' });
  assert.throws(() => applyAtomically(failedDuring, finishPlan, NOW + 31), { code: 'MODEL_CAS_CONFLICT' });
  const closed = applyAtomically(failedDuring, planFinishValidate(failedDuring,
    { ...finish, result: 'coordination_aborted' }, NOW + 31), NOW + 31).records;
  assert.equal(closed['B-validate'].state, 'coordination_aborted'); assert(!closed['B-authorization']);
});

test('failure after authorization blocks first Consume while cancellation and consumed transaction convergence remain usable', () => {
  const { current, begin, finish, consume, failure } = preparedHandoffFixture();
  const coordinated = applyAtomically(current, planBeginValidate(current, begin, NOW + 30), NOW + 30).records;
  const authorized = applyAtomically(coordinated, planFinishValidate(coordinated, finish, NOW + 31), NOW + 31).records;
  authorized['consume-operation'] = operation(NOW + 32);
  authorized['B-authorization'].consumeReservationKey = 'B-reservation';
  authorized['B-reservation'] = { revision: 1, request: consume.request, authorizationJti: 'jti-1',
    transactionKey: 'B-transaction', outcomeKey: 'B-outcome', timeoutKey: 'B-transaction-timeout', diagnosticCursorKey: 'B-transaction-diagnostic' };
  const plan = planConsume(authorized, consume, NOW + 32); const failed = failure(authorized, NOW + 32);
  assert.throws(() => planConsume(failed, consume, NOW + 32), { code: 'MODEL_WORKFLOW_ENDED' });
  assert.throws(() => applyAtomically(failed, plan, NOW + 32), { code: 'MODEL_CAS_CONFLICT' });
  const cancelled = applyAtomically(failed, planAuthorizationEnd(failed, { ...consume, action: 'cancel' }, NOW + 32), NOW + 32).records;
  assert.equal(cancelled['B-outcome'].state, 'cancelled'); assert(!cancelled['B-transaction']);
  const consumed = applyAtomically(authorized, plan, NOW + 32).records; const ended = failure(consumed, NOW + 33);
  assert.equal(ended.workflow.state, 'failed'); assert.equal(ended['B-transaction'].state, 'authorization_consumed');
  assert.doesNotThrow(() => assertTransactionTransition(ended['B-transaction'], 'execution_ready', { now: NOW + 34, facts: modelFacts() }));
});
