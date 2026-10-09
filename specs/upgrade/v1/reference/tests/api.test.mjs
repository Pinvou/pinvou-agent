import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { API_SCHEMAS, OPERATIONS, openapi } from '../build-api.mjs';
import { assertApiContract, assertApiShape } from '../api-registry.mjs';
import { unactivatedCheck, errorResponse } from '../api-errors.mjs';
import { canonicalize, parseJson } from '../canonical-json.mjs';
import { sha256, consumeRequestDigest } from '../digests.mjs';
import { createFixtureSet, NOW, HASH, TARGET } from './fixtures.mjs';
import { consumeFixture } from './model-fixtures.mjs';

const scope = { product: 'pinvou', component: 'app', installId: 'install-1', installationScopeId: 'scope-1',
  channel: 'stable', channelRevision: 1, targetKey: TARGET };

test('artifact drafts bind each Git source algorithm to its exact digest length', () => {
  const request = { protocolVersion: 1,
    scope: { product: 'pinvou', component: 'app', channel: 'stable', targetKey: TARGET },
    commandKey: 'command-1', reason: 'publish-artifact',
    expectedReadSet: [{ recordKind: 'artifact', recordId: 'artifact-1', revision: 1, sha256: HASH }],
    draft: { artifactKind: 'full-package', artifact: { size: 123, sha256: HASH },
      targetKey: TARGET, appVersion: '2.0.0', sourceRevision: null } };
  for (const [algorithm, length] of [['git-sha1', 40], ['git-sha256', 64]]) {
    request.draft.sourceRevision = { algorithm, value: 'a'.repeat(length) };
    assertApiContract('create-artifact-draft', 'request', request);
    request.draft.sourceRevision.value = 'a'.repeat(length === 40 ? 64 : 40);
    assert.throws(() => assertApiContract('create-artifact-draft', 'request', request),
      { code: 'API_CONTRACT_INVALID' });
  }
});

test('committed OpenAPI and every closed API schema exactly match deterministic generators', async () => {
  assert.equal(await readFile(new URL('../../openapi.yaml', import.meta.url), 'utf8'), JSON.stringify(openapi, null, 2) + '\n');
  for (const [name, schema] of API_SCHEMAS) assert.equal(await readFile(new URL(`../../schemas/api/${name}.json`, import.meta.url), 'utf8'),
    JSON.stringify(schema, null, 2) + '\n', name);
  const referenceTargets = new Set(Object.keys(openapi.components.schemas));
  function visit(value) {
    if (value === null || typeof value !== 'object') return;
    if (value.$ref !== undefined) {
      assert(value.$ref.startsWith('#/components/schemas/')); const parts = value.$ref.slice(2).split('/');
      let target = openapi; for (const part of parts) { assert(Object.hasOwn(target, part), value.$ref); target = target[part]; }
    }
    for (const item of Object.values(value)) visit(item);
  }
  visit(openapi); assert(referenceTargets.size > API_SCHEMAS.size);
  assert.equal(new Set(OPERATIONS.map((operation) => operation.path)).size, OPERATIONS.length);
});
test('unactivated scope is a fixed closed 409 with no signed result or lineage', () => {
  assert.deepEqual(unactivatedCheck('request-1', 60_000), { status: 409, body: { protocolVersion: 1,
    requestId: 'request-1', code: 'SCOPE_UNACTIVATED', retryable: true, checkAfterMs: 60_000 } });
  assert.throws(() => assertApiShape('unactivated', { ...unactivatedCheck('request-1', 60_000).body, decision: 'secret' }));
  assert.throws(() => assertApiShape('error', { protocolVersion: 1, requestId: 'request-1', code: 'SCOPE_UNACTIVATED', retryable: true }));
});
test('stable errors contain only frozen fields and retry policy', () => {
  assert.deepEqual(errorResponse('UNKNOWN_CONSUME_OUTCOME', 'request-1'), { status: 404, body: { protocolVersion: 1,
    requestId: 'request-1', code: 'UNKNOWN_CONSUME_OUTCOME', retryable: false } });
  assert.throws(() => errorResponse('EVENT_KEY_DENIED', 'request-1', 1000));
  assert.throws(() => errorResponse('private-secret-from-exception', 'request-1'));
});
test('check contract is closed and production incremental capabilities are empty', () => {
  const set = createFixtureSet();
  const request = { protocolVersion: 1, scope, requestNonce: 'nonce-1', executionPurpose: 'install',
    grayTargeting: { mode: 'disabled', serialNumber: null },
    facts: { stableFacts: set.sourceFacts, host: set.claims.decision.host, clientFactsDigest: HASH,
      updaterFactsDigest: HASH, helperFactsDigest: HASH, launcherFactsDigest: HASH },
    supportedIncrementalAlgorithms: [], supportedIncrementalFormats: [] };
  assertApiContract('check', 'request', request);
  assert.throws(() => assertApiContract('check', 'request', { ...request, grayTargeting: { mode: 'disabled', serialNumber: 'test-only-sn' } }));
  assert.throws(() => assertApiContract('check', 'request', { ...request, secret: 'must-not-log' }), { code: 'API_CONTRACT_INVALID' });
  assert.throws(() => assertApiContract('check', 'request', { ...request, supportedIncrementalAlgorithms: ['bsdiff'] }), { code: 'INCREMENTAL_NOT_ENABLED' });
});
test('all three consume routes and status load one shared contract without route aliasing', () => {
  for (const purpose of ['preinstall', 'install', 'activate']) {
    const { command } = consumeFixture(purpose); const { consumeRequestDigest: digest, ...semanticRequest } = command.request;
    const request = { protocolVersion: 1, scope, authorization: parseJson(command.authorizationBytes), authorizationRevision: 1,
      semanticRequest, consumeRequestDigest: digest };
    assertApiContract(`consume-${purpose}`, 'request', request);
    const wrong = structuredClone(request); wrong.semanticRequest.purpose = purpose === 'install' ? 'activate' : 'install';
    wrong.consumeRequestDigest = consumeRequestDigest(wrong.semanticRequest);
    assert.throws(() => assertApiContract(`consume-${purpose}`, 'request', wrong), { code: 'API_CONSUME_MISMATCH' });
  }
  const status = { protocolVersion: 1, product: 'pinvou', component: 'app', installationScopeId: 'scope-1', purpose: 'install',
    authorizationJti: 'jti-1', consumeKey: 'consume-1', transactionId: 'transaction-1', consumeRequestDigest: HASH,
    recoverySecret: Buffer.alloc(32).toString('base64url') };
  assertApiContract('consume-status', 'request', status);
  assert.throws(() => assertApiContract('consume-status', 'request', { ...status, recoverySecret: 'A'.repeat(42) + 'B' }));
  assert.throws(() => assertApiContract('consume-status', 'response', { protocolVersion: 1, requestId: 'request-1', state: 'cancelled', transactionId: 'wrong' }));
});
test('event role, purpose, lineage, occurrence and actual facts digest agree', () => {
  const set = createFixtureSet(); const claims = set.claims['preinstall-transaction-event'];
  const credential = parseJson(set.sign(claims)); const facts = { factType: 'local-state' };
  const event = { eventId: 'event-1', sequence: 1, occurredAt: NOW + 1, lineageId: 'transaction-1', executionPurpose: 'preinstall',
    kind: 'authorization_consumed', fromState: 'authorization_consumed', toState: 'authorization_consumed',
    facts, factsSha256: sha256(canonicalize(facts)), targetIdentitySha256: HASH };
  const request = { protocolVersion: 1, scope, credential, events: [event] };
  assertApiContract('events-preinstall-transaction-event', 'request', request);
  for (const changed of [{ ...event, executionPurpose: 'install' }, { ...event, lineageId: 'session-1' },
    { ...event, occurredAt: NOW + 30 * 86_400_000 }, { ...event, factsSha256: 'b'.repeat(64) }])
    assert.throws(() => assertApiContract('events-preinstall-transaction-event', 'request', { ...request, events: [event, changed] }),
      (error) => ['API_CONTRACT_INVALID', 'API_EVENT_MISMATCH'].includes(error.code));
});
test('responses and dedicated actions reject cross-lineage bindings while accepting legal future nbf', () => {
  const set = createFixtureSet();
  set.claims.decision.nbf = NOW + 1;
  const decision = parseJson(set.sign(set.claims.decision)); const telemetryCredential = parseJson(set.sign(set.claims['telemetry-event']));
  const response = { protocolVersion: 1, requestId: 'request-1', decision, telemetryCredential };
  assertApiContract('check', 'response', response);
  const crossed = structuredClone(response); crossed.telemetryCredential.signed.telemetrySessionId = 'other-session';
  assert.throws(() => assertApiContract('check', 'response', crossed), { code: 'API_RESPONSE_MISMATCH' });
  const credential = parseJson(set.sign(set.claims.download));
  const locator = { protocolVersion: 1, requestId: 'request-1', credential, packageId: credential.signed.packageId,
    size: credential.signed.package.size, sha256: credential.signed.package.sha256, url: 'https://example.test/package',
    expiresAt: credential.signed.exp, fileSourceClass: 'primary' };
  assertApiContract('download-info', 'response', locator);
  assert.throws(() => assertApiContract('download-info', 'response', { ...locator, expiresAt: credential.signed.exp + 1 }), { code: 'API_RESPONSE_MISMATCH' });
  const authorization = parseJson(set.sign(set.claims['authorization-install'])); const body = authorization.signed;
  const facts = { stableFacts: set.sourceFacts, host: body.host, clientFactsDigest: HASH,
    updaterFactsDigest: HASH, helperFactsDigest: HASH, launcherFactsDigest: HASH };
  const review = { protocolVersion: 1, scope, authorization, transactionId: body.transactionId, transactionRevision: 1,
    facts, verifiedObjectSha256: HASH, preparation: body.preparation, freezeEpoch: body.freezeEpoch,
    backup: body.backup, staged: body.staged, preinstallSlot: body.preinstallSlot };
  assertApiContract('execution-review-install', 'request', review);
  assert.throws(() => assertApiContract('execution-review-install', 'request', { ...review, transactionId: 'other-tx' }), { code: 'API_PURPOSE_MISMATCH' });
  assert.throws(() => assertApiContract('execution-review-install', 'request', { ...review, freezeEpoch: 2 }), { code: 'API_PURPOSE_MISMATCH' });
  const taskCredential = parseJson(set.sign(set.claims['reconciliation-action'])); const evidence = { factType: 'local-state' };
  const action = { protocolVersion: 1, scope, actionKey: 'action-1', credential: taskCredential,
    taskRevision: taskCredential.signed.task.revision, evidence, evidenceSha256: sha256(canonicalize(evidence)) };
  assertApiContract('reconciliation-action', 'request', action);
  assert.throws(() => assertApiContract('reconciliation-action', 'request', { ...action, taskRevision: action.taskRevision + 1 }), { code: 'API_EVIDENCE_MISMATCH' });
  const cancel = { protocolVersion: 1, scope, authorization, cancelKey: 'cancel-1', authorizationJti: body.authorizationJti,
    authorizationRevision: 1, reason: 'user-stopped' };
  assertApiContract('cancel-install', 'request', cancel);
  assert.throws(() => assertApiContract('cancel-install', 'request', { ...cancel, authorization: decision }));
});
