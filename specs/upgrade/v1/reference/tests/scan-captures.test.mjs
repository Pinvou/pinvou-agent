import test from 'node:test';
import assert from 'node:assert/strict';
import { API_SCHEMAS, OPERATIONS, openapi } from '../build-api.mjs';
import { assertApiContract, assertApiShape } from '../api-registry.mjs';

const scope = { product: 'pinvou', component: 'desktop', channel: 'stable', targetKey: 'linux-x86_64-deb-perMachine' };
const request = { protocolVersion: 1, commandKey: 'original-scan-capture', scope,
  material: { kind: 'supplyChainMaterial', id: 'material-' + 'a'.repeat(64), revision: 1, sha256: 'b'.repeat(64) }, templateSHA256: 'c'.repeat(64) };
const queued = { protocolVersion: 1, resultId: 'd'.repeat(64), state: 'queued', queuedAt: 100, originalSHA256: 'e'.repeat(64) };
const response = result => ({ protocolVersion: 1, requestId: 'scan-capture-response', result });
const observation = { protocolVersion: 1, completedAt: 200, coverage: 'reported-only-unassessed', sealSHA256: 'f'.repeat(64),
  report: { name: 'report.json', size: 123, sha256: '1'.repeat(64) }, normalized: { name: 'normalized.json', size: 321, sha256: '2'.repeat(64) } };

test('capture contracts register only four fixed metadata routes with exact request and response aliases', () => {
  const actual = OPERATIONS.filter(operation => operation.path.startsWith('/v1/admin/supply-chain/scan-captures/'));
  assert.deepEqual(actual.map(operation => operation.name), ['context', 'queue', 'lookup', 'resume'].map(route => 'scan-capture-' + route));
  for (const operation of actual) {
    const route = operation.name.slice('scan-capture-'.length), post = openapi.paths[operation.path].post;
    assert.equal(operation.owner, 'T10'); assert.equal(operation.management, true);
    assert.equal(operation.requestShape, `scan-capture.${route}-request`);
    assert.equal(operation.responseShape, `scan-capture.${route}-response`);
    assert.equal(API_SCHEMAS.has(operation.requestShape), true); assert.equal(API_SCHEMAS.has(operation.responseShape), true);
    assert.deepEqual(post.parameters.map(p => p.name), ['Origin', 'X-CSRF-Token']);
    assert.equal(post.security.length, 2); assert.equal(post['x-fresh-mfa-required'], false);
    assert.equal(Object.keys(post.responses).some(status => status === '202'), false);
  }
});

test('capture requests bind a concrete material revision and template without accepting execution controls', () => {
  assertApiContract('scan-capture-context', 'request', { protocolVersion: 1, scope });
  for (const operation of ['queue', 'lookup', 'resume']) {
    const name = 'scan-capture-' + operation; assertApiContract(name, 'request', request);
    for (const changed of [{ ...request, actorId: 'caller-asserted' }, { ...request, scannerPath: '/arbitrary/path' },
      { ...request, material: { ...request.material, kind: 'supplyChainScan' } }, { ...request, material: { ...request.material, revision: 0 } },
      { ...request, material: { ...request.material, revision: null } }, { ...request, material: { ...request.material, id: 'freeform' } },
      { ...request, scope: { ...scope, component: null } }, { ...request, templateSHA256: 'C'.repeat(64) }]) {
      assert.throws(() => assertApiContract(name, 'request', changed), { code: 'API_CONTRACT_INVALID' });
    }
  }
});

test('capture transport queue acknowledgement cannot masquerade as a completed scan', () => {
  assertApiContract('scan-capture-context', 'response', response({ protocolVersion: 1, scope, templateSHA256: request.templateSHA256, admission: true }));
  assertApiContract('scan-capture-queue', 'response', response(queued));
  for (const changed of [{ ...queued, state: 'recorded' }, { ...queued, observation }, { ...queued, queuedAt: 0 }, { ...queued, queuedAt: 9007199254740992 },
    { ...queued, resultId: 'not-a-result-hash' }]) assert.throws(() => assertApiContract('scan-capture-queue', 'response', response(changed)), { code: 'API_CONTRACT_INVALID' });
  assert.throws(() => assertApiContract('scan-capture-queue', 'response', queued), { code: 'API_CONTRACT_INVALID' });
});

test('capture lookup and resume preserve unresolved and unfinished states with closed reported-only observations', () => {
  for (const operation of ['scan-capture-lookup', 'scan-capture-resume']) {
    assertApiContract(operation, 'response', response({ protocolVersion: 1, state: 'unresolved', queued: null, observation: null }));
    for (const state of ['queued', 'launch_authorized', 'unknown']) assertApiContract(operation, 'response', response({ protocolVersion: 1, state, queued, observation: null }));
    assertApiContract(operation, 'response', response({ protocolVersion: 1, state: 'recorded', queued, observation }));
    for (const changed of [{ protocolVersion: 1, state: 'unresolved', queued, observation: null }, { protocolVersion: 1, state: 'queued', queued, observation },
      { protocolVersion: 1, state: 'recorded', queued, observation: null },
      { protocolVersion: 1, state: 'recorded', queued, observation: { ...observation, coverage: 'formal' } },
      { protocolVersion: 1, state: 'not_committable', queued: null, observation: null },
      { protocolVersion: 1, state: 'recorded', queued, observation: { ...observation, report: { ...observation.report, name: '../report.json' } } }]) {
      assert.throws(() => assertApiContract(operation, 'response', response(changed)), { code: 'API_CONTRACT_INVALID' });
    }
  }
});

test('capture machine errors remain the existing four-field envelope without terminal or provider details', () => {
  const value = { protocolVersion: 1, requestId: 'original-lookup', code: 'REVISION_CONFLICT', retryable: true };
  assertApiShape('scan-capture.error', value);
  for (const changed of [{ ...value, terminal: { disposition: 'not_committable' } }, { ...value, retryAfterMs: 5 },
    { ...value, sqlstate: '40001' }, { ...value, retryable: false }, { ...value, code: 'OWNER_FENCED' }]) {
    assert.throws(() => assertApiShape('scan-capture.error', changed), { code: 'API_CONTRACT_INVALID' });
  }
});

test('capture approval-required errors use existing HTTP 403 and a nonretryable envelope', () => {
  for (const route of ['context', 'queue', 'lookup', 'resume']) {
    const responses = openapi.paths['/v1/admin/supply-chain/scan-captures/' + route].post.responses;
    const codes = status => responses[status].content['application/json'].schema.allOf[1].properties.code.enum;
    assert.equal(codes('403').includes('APPROVAL_REQUIRED'), true);
    assert.equal(codes('409').includes('APPROVAL_REQUIRED'), false);
  }
  const error = { protocolVersion: 1, requestId: 'approval-required', code: 'APPROVAL_REQUIRED', retryable: false };
  assertApiShape('scan-capture.error', error);
  assert.throws(() => assertApiShape('scan-capture.error', { ...error, retryable: true }), { code: 'API_CONTRACT_INVALID' });
});

test('capture output filenames reject final line terminators in the schema engine', () => {
  for (const name of ['report.json\n', 'report.json\r', 'report.json\r\n', 'report.json\u2028', 'report.json\u2029']) {
    assert.throws(() => assertApiShape('scan-capture.output', { ...observation.report, name }), { code: 'API_CONTRACT_INVALID' });
  }
  assertApiShape('scan-capture.output', observation.report);
});
