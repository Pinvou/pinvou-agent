import test from 'node:test';
import assert from 'node:assert/strict';
import { ENTITY_EDGES, SESSION_EDGES, TRANSACTION_EDGES, assertEntityEdge, assertEdge } from '../models/state-graphs.mjs';
import { assertSessionTransition, sessionTimeoutState } from '../models/session.mjs';
import { assertTransactionTransition, transactionTimeoutState, transactionTransition } from '../models/transaction.mjs';
import { modelSession, modelTransaction, modelFacts } from './model-fixtures.mjs';
import { DAY } from '../semantics.mjs';
import { NOW } from './fixtures.mjs';
import { readFile } from 'node:fs/promises';
const vectors = JSON.parse(await readFile(new URL('../../vectors/states/ordinary-edges.json', import.meta.url), 'utf8'));

test('ordinary state graphs match the committed normative table independent of runtime construction', () => {
  assert.deepEqual(ENTITY_EDGES, vectors.entities); assert.deepEqual(SESSION_EDGES, vectors.sessions);
  assert.deepEqual(TRANSACTION_EDGES, vectors.transactions);
});

test('every entity graph accepts exactly its declared single-step edges', () => {
  for (const [kind, graph] of Object.entries(vectors.entities)) {
    const states = new Set([...Object.keys(graph), ...Object.values(graph).flat(), 'unknown']);
    for (const from of states) for (const to of states) {
      const valid = graph[from]?.includes(to) ?? false;
      if (valid) assert.doesNotThrow(() => assertEntityEdge(kind, from, to));
      else assert.throws(() => assertEntityEdge(kind, from, to), { code: 'MODEL_EDGE_INVALID' });
    }
  }
});
test('every declared session edge has an eligible purpose/type scenario', () => {
  for (const [from, next] of Object.entries(vectors.sessions)) for (const to of next) {
    let purpose = from === 'staged_verified' || to === 'staged_verified' ? 'activate' : 'install';
    if (from === 'permission_granted' && to === 'preparation_ready') purpose = 'preinstall';
    const session = modelSession(from, purpose); const facts = modelFacts();
    if (to === 'download_resume_context') {
      session.immutableContextSha256 = 'a'.repeat(64);
      session.downloadResumeBinding = { workflowId: session.workflowId, fromSessionId: 'previous-session',
        toSessionId: session.sessionId, hopSha256: session.hopSha256, immutableContextSha256: session.immutableContextSha256,
        confirmedHopSha256: session.hopSha256, handedOffAt: NOW };
    }
    if (['update_offered', 'deferred'].includes(from) && to === 'download_started') session.upgradeType = 'forced';
    if (['backup_started', 'backup_succeeded'].includes(from) || ['backup_started', 'backup_succeeded'].includes(to)) session.backupPolicy = 'required';
    assert.doesNotThrow(() => assertSessionTransition(session, to, { now: NOW + 1, actor: 'lifecycle', facts }), `${from}->${to}`);
  }
});
test('every undeclared ordinary session edge and every unknown source is rejected', () => {
  const states = new Set([...Object.keys(vectors.sessions), ...Object.values(vectors.sessions).flat(), 'unknown']);
  for (const from of states) for (const to of states) if (!vectors.sessions[from]?.includes(to))
    assert.throws(() => assertSessionTransition(modelSession(from), to, { now: NOW + 1, actor: 'lifecycle', facts: modelFacts() }));
});
test('each purpose transaction graph accepts every legal edge and rejects all others', () => {
  for (const [purpose, graph] of Object.entries(vectors.transactions)) {
    const states = new Set([...Object.keys(graph), ...Object.values(graph).flat(), 'unknown']);
    for (const from of states) for (const to of states) {
      const transaction = modelTransaction(from, purpose); const facts = modelFacts();
      if (to === 'helper_plan_started' || from === 'helper_plan_started') transaction.helperPlanRef = { planId: 'helper-plan', revision: 1, sha256: 'a'.repeat(64) };
      const operation = () => assertTransactionTransition(transaction, to, { now: NOW + 1, facts });
      if (graph[from]?.includes(to)) assert.doesNotThrow(operation, `${purpose}:${from}->${to}`);
      else assert.throws(operation);
    }
  }
});
test('server state, activity, confirmation and backup cannot be fabricated across purposes', () => {
  const transition = (session, to, facts = modelFacts(), actor = 'client') => assertSessionTransition(session, to, { now: NOW + 1, facts, actor });
  assert.throws(() => transition(modelSession('authorization_requested'), 'authorized'), { code: 'MODEL_SERVER_ONLY' });
  assert.throws(() => transition(modelSession('update_offered'), 'download_started'), { code: 'MODEL_CONFIRMATION_REQUIRED' });
  assert.throws(() => transition(modelSession('permission_granted', 'preinstall'), 'preparation_started'));
  const required = { ...modelSession('backup_started'), backupPolicy: 'required', freezeEpoch: null };
  assert.throws(() => transition(required, 'backup_succeeded', { ...modelFacts(), backupFreezeEpoch: null }));
  assert.throws(() => transition(modelSession('unknown'), 'cancelled', { cause: 'cancelled', consumeState: 'not_sent' }, 'lifecycle'));
});
test('execution boundaries persist and cannot become safe cancellation', () => {
  const ready = modelTransaction('execution_ready');
  const started = transactionTransition(ready, 'installer_started', { now: NOW + 1, facts: modelFacts() });
  assert.equal(started.boundaryAt, NOW + 1);
  assert.throws(() => assertTransactionTransition(started, 'abandoned_before_install', { now: NOW + 2, facts: modelFacts() }));
  assert.throws(() => assertTransactionTransition({ ...started, boundaryAt: null }, 'abandoned_before_install', { now: NOW + 2, facts: modelFacts() }));
  assert.throws(() => assertTransactionTransition(modelTransaction('unknown', 'preinstall'), 'staging_failed', { now: NOW + 1, facts: modelFacts() }));
  for (const purpose of ['install', 'activate']) assert.throws(() => assertTransactionTransition(modelTransaction('authorization_consumed', purpose),
    purpose === 'install' ? 'abandoned_before_install' : 'cancelled_before_install', { now: NOW + 1, facts: { ...modelFacts(), activeMutationCount: 1 } }));
  assert.throws(() => assertTransactionTransition(modelTransaction('health_check_started'), 'succeeded', { now: NOW + 1,
    facts: { ...modelFacts(), healthStartedAt: NOW + 2 } }), { code: 'MODEL_HEALTH_INVALID' });
});
test('24-hour/30-day equality expires; terminal state remains immutable', () => {
  for (const offset of [-1, 0, 1]) {
    assert.equal(sessionTimeoutState(modelSession(), NOW + DAY + offset), offset < 0 ? 'authorized' : 'authorization_expired');
    for (const purpose of ['preinstall', 'install', 'activate']) assert.equal(transactionTimeoutState(modelTransaction('authorization_consumed', purpose), NOW + 30 * DAY + offset),
      offset < 0 ? 'authorization_consumed' : purpose === 'preinstall' ? 'staging_failed' : 'failed_manual_repair_required');
  }
  assert.equal(transactionTimeoutState(modelTransaction('succeeded'), NOW + 38 * DAY), 'succeeded');
  assert.equal(sessionTimeoutState(modelSession('authorization_consumed'), NOW + 38 * DAY), 'authorization_consumed');
});
