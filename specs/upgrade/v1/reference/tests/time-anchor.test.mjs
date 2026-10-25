import test from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { canonicalize, parseJson } from '../canonical-json.mjs';
import { sha256 } from '../digests.mjs';
import { publicKeyDescriptor, signEnvelope, verifyEnvelope, verifyTimeResponse } from '../signatures.mjs';
import { assertTimeRequest, assertTimeClaims } from '../time-contract.mjs';
import { assertApiContract } from '../api-registry.mjs';
import { openapi } from '../build-api.mjs';
import { createTimeVectors } from '../generate-time-vectors.mjs';
import { createFixtureSet, NOW } from './fixtures.mjs';
import { DAY } from '../semantics.mjs';

const rejects = (fn, code) => assert.throws(fn, code === undefined ? undefined : { code });
const plain = (value) => JSON.parse(JSON.stringify(value));
const compare = (left, right) => left < right ? -1 : left > right ? 1 : 0;

function vectorContext(vector) {
  return { request: vector.request, trustedRootBytes: Buffer.from(vector.trustedRootBytes),
    rootSuccessors: vector.rootSuccessors.map((value) => Buffer.from(value)),
    sentMonotonicMs: vector.sentMonotonicMs, receivedMonotonicMs: vector.receivedMonotonicMs,
    maxRoundTripMs: vector.maxRoundTripMs, maxUncertaintyMs: vector.maxUncertaintyMs,
    trustedTimeFloor: vector.trustedTimeFloor, deniedKeyIds: vector.deniedKeyIds };
}

function fixture() {
  const set = createFixtureSet();
  const timeKeys = Array.from({ length: 2 }, () => generateKeyPairSync('ed25519').privateKey);
  const root = structuredClone(set.metadata.root);
  root.keys.push(...timeKeys.map(publicKeyDescriptor));
  root.roles.push({ role: 'time', component: 'app', channel: null, targetKey: null,
    threshold: 2, keyIds: timeKeys.map((key) => publicKeyDescriptor(key).keyId) });
  const request = { protocolVersion: 1, product: 'pinvou', component: 'app',
    requestNonce: Buffer.alloc(32, 7).toString('base64url'), root: null };
  const claims = { protocolVersion: 1, role: 'time', product: 'pinvou', component: 'app', scope: {},
    purpose: 'trusted-time.v1', requestNonce: request.requestNonce, root: null,
    serverTime: NOW + 100, uncertaintyMs: 5 };
  const context = { request, trustedRootBytes: null, rootSuccessors: [],
    sentMonotonicMs: 1000, receivedMonotonicMs: 1020,
    maxRoundTripMs: 20, maxUncertaintyMs: 5, trustedTimeFloor: NOW, deniedKeyIds: [] };
  function bindRoot() {
    context.trustedRootBytes = signEnvelope(root, set.keys.slice(0, 2));
    request.root = { version: root.version, sha256: sha256(context.trustedRootBytes) };
    claims.root = { ...request.root };
  }
  bindRoot();
  return { set, timeKeys, root, request, claims, context, bindRoot,
    bytes: () => signEnvelope(claims, timeKeys) };
}

test('time response yields only immutable interval DATA at original monotonic receive', () => {
  const f = fixture(), bytes = f.bytes();
  const before = Buffer.from(canonicalize(f.request));
  const result = verifyTimeResponse(bytes, f.context);
  assert.deepEqual(result, { earliest: NOW + 95, latest: NOW + 125,
    asOfMonotonicMs: 1020, root: f.request.root });
  assert(Object.isFrozen(result)); assert(Object.isFrozen(result.root));
  assert.deepEqual(Buffer.from(canonicalize(f.request)), before);
  assert.deepEqual(Object.keys(result).sort(compare), ['asOfMonotonicMs', 'earliest', 'latest', 'root']);
  assert.equal(assertTimeRequest(f.request), f.request);
  assert.equal(assertTimeClaims(f.claims), f.claims);
});

test('time wire is closed and every mandatory field remains mandatory', () => {
  const f = fixture();
  for (const field of Object.keys(f.request)) {
    const request = structuredClone(f.request); delete request[field];
    rejects(() => assertTimeRequest(request), 'SCHEMA_INVALID');
  }
  for (const field of Object.keys(f.claims)) {
    const envelope = parseJson(f.bytes()); delete envelope.signed[field];
    rejects(() => verifyTimeResponse(canonicalize(envelope), f.context), 'SCHEMA_INVALID');
  }
  for (const field of ['iat', 'nbf', 'exp', 'trusted', 'requestId']) {
    const envelope = parseJson(f.bytes()); envelope.signed[field] = 1;
    rejects(() => verifyTimeResponse(canonicalize(envelope), f.context), 'SCHEMA_INVALID');
  }
  for (const field of ['product', 'component']) {
    for (const value of ['', 'a'.repeat(129), 'app\n', 'app\r', 'app\u2028', 'app\u2029']) {
      const request = { ...f.request, [field]: value };
      const claims = { ...f.claims, [field]: value };
      rejects(() => assertTimeRequest(request), 'SCHEMA_INVALID');
      rejects(() => assertApiContract('time', 'request', request), 'API_CONTRACT_INVALID');
      rejects(() => assertTimeClaims(claims), 'SCHEMA_INVALID');
      rejects(() => assertApiContract('time', 'response', { signed: claims,
        signatures: parseJson(f.bytes()).signatures }), 'API_CONTRACT_INVALID');
    }
    const value = 'a'.repeat(128);
    assertTimeRequest({ ...f.request, [field]: value });
    assertTimeClaims({ ...f.claims, [field]: value });
  }
  rejects(() => assertTimeRequest({ ...f.request, now: NOW }), 'SCHEMA_INVALID');
  rejects(() => assertTimeRequest({ ...f.request, root: { ...f.request.root, size: 1 } }), 'SCHEMA_INVALID');
});

test('nonce is exactly canonical 32-byte base64url in request, API and signed claims', () => {
  const f = fixture();
  for (const nonce of ['A'.repeat(42), 'A'.repeat(44), 'A'.repeat(42) + 'B',
    f.request.requestNonce + '=', f.request.requestNonce + '\n', ' '.repeat(43)]) {
    rejects(() => assertTimeRequest({ ...f.request, requestNonce: nonce }));
    rejects(() => assertApiContract('time', 'request', { ...f.request, requestNonce: nonce }));
    rejects(() => assertTimeClaims({ ...f.claims, requestNonce: nonce }));
  }
  const nonce = Buffer.alloc(32, 255).toString('base64url');
  assertTimeRequest({ ...f.request, requestNonce: nonce });
  f.claims.requestNonce = nonce;
  rejects(() => verifyTimeResponse(f.bytes(), f.context), 'TIME_REQUEST_BINDING_INVALID');
});

test('all policy, protected floor and original observation inputs are explicit and safe', () => {
  const f = fixture(), bytes = f.bytes();
  for (const field of ['request', 'trustedRootBytes', 'sentMonotonicMs', 'receivedMonotonicMs',
    'maxRoundTripMs', 'maxUncertaintyMs', 'trustedTimeFloor', 'deniedKeyIds']) {
    const context = { ...f.context }; delete context[field];
    rejects(() => verifyTimeResponse(bytes, context), 'TIME_CONTEXT_REQUIRED');
  }
  for (const field of ['sentMonotonicMs', 'receivedMonotonicMs', 'maxRoundTripMs',
    'maxUncertaintyMs', 'trustedTimeFloor']) {
    for (const value of [-1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1, '0']) {
      rejects(() => verifyTimeResponse(bytes, { ...f.context, [field]: value }), 'TIME_CONTEXT_INVALID');
    }
  }
  rejects(() => verifyTimeResponse(bytes, { ...f.context, now: NOW }), 'TIME_CONTEXT_REQUIRED');
  rejects(() => verifyTimeResponse(bytes, { ...f.context, receivedMonotonicMs: 999 }), 'TIME_MONOTONIC_INVALID');
  rejects(() => verifyTimeResponse(bytes, { ...f.context, maxRoundTripMs: 19 }), 'TIME_RTT_EXCEEDED');
  rejects(() => verifyTimeResponse(bytes, { ...f.context, maxUncertaintyMs: 4 }), 'TIME_UNCERTAINTY_EXCEEDED');
  rejects(() => verifyTimeResponse(bytes, { ...f.context, trustedTimeFloor: NOW + 96 }), 'TIME_ROLLBACK');
  for (const deniedKeyIds of [null, ['unknown'], ['a'.repeat(64) + '\n'], ['a'.repeat(64), 'a'.repeat(64)]]) {
    rejects(() => verifyTimeResponse(bytes, { ...f.context, deniedKeyIds }), 'TIME_DENY_SET_INVALID');
  }
  for (const rootSuccessors of [null, {}, Array(4097).fill(new Uint8Array())]) {
    rejects(() => verifyTimeResponse(bytes, { ...f.context, rootSuccessors }), 'ROOT_CHAIN_INVALID');
  }
  f.claims.uncertaintyMs = 0;
  const zero = verifyTimeResponse(f.bytes(), { ...f.context, receivedMonotonicMs: 1000,
    maxRoundTripMs: 0, maxUncertaintyMs: 0 });
  assert.equal(zero.earliest, NOW + 100); assert.equal(zero.latest, NOW + 100);
});

test('request and response bind the exact full final Root Envelope, version and context', () => {
  const f = fixture();
  for (const root of [{ ...f.request.root, sha256: sha256(canonicalize(f.root)) },
    { ...f.request.root, version: f.root.version + 1 }]) {
    rejects(() => verifyTimeResponse(f.bytes(), { ...f.context,
      request: { ...f.request, root } }), 'TIME_ROOT_BINDING_INVALID');
  }
  f.claims.root.sha256 = 'b'.repeat(64);
  rejects(() => verifyTimeResponse(f.bytes(), f.context), 'TIME_REQUEST_BINDING_INVALID');
  f.claims.root = { ...f.request.root };
  for (const field of ['product', 'component']) {
    const changed = { ...f.claims, [field]: 'other' };
    rejects(() => verifyTimeResponse(signEnvelope(changed, f.timeKeys), f.context), 'CONTEXT_MISMATCH');
  }
  f.root.version++;
  const changedBytes = signEnvelope(f.root, f.set.keys.slice(0, 2));
  rejects(() => verifyTimeResponse(f.bytes(), { ...f.context, trustedRootBytes: changedBytes }),
    'TIME_ROOT_BINDING_INVALID');
});

test('dedicated time role signatures enforce threshold, encoding, ordering and known deny', () => {
  const f = fixture();
  rejects(() => verifyTimeResponse(signEnvelope(f.claims, [f.set.keys[3]]), f.context), 'SIGNING_KEY_NOT_AUTHORIZED');
  rejects(() => verifyTimeResponse(signEnvelope(f.claims, [f.timeKeys[0]]), f.context), 'SIGNATURE_THRESHOLD_UNMET');
  const envelope = parseJson(f.bytes());
  const duplicate = { ...envelope, signatures: [envelope.signatures[0], envelope.signatures[0]] };
  rejects(() => verifyTimeResponse(canonicalize(duplicate), f.context), 'SIGNATURE_ORDER_INVALID');
  rejects(() => verifyTimeResponse(canonicalize({ ...envelope, signatures: [...envelope.signatures].reverse() }), f.context),
    'SIGNATURE_ORDER_INVALID');
  const bad = plain(envelope); bad.signatures[0].signature = 'A'.repeat(86);
  rejects(() => verifyTimeResponse(canonicalize(bad), f.context), 'SIGNATURE_INVALID');
  bad.signatures[0].signature = 'A'.repeat(85) + 'B';
  rejects(() => verifyTimeResponse(canonicalize(bad), f.context), 'SIGNATURE_ENCODING_INVALID');
  rejects(() => verifyTimeResponse(f.bytes(), { ...f.context,
    deniedKeyIds: [publicKeyDescriptor(f.timeKeys[0]).keyId] }), 'SIGNING_KEY_DENIED');
  rejects(() => verifyTimeResponse(signEnvelope({ ...f.claims, serverTime: NOW + 101 }, f.timeKeys),
    { ...f.context, deniedKeyIds: [publicKeyDescriptor(f.timeKeys[1]).keyId] }), 'SIGNING_KEY_DENIED');
  const forged = plain(envelope); forged.signed.serverTime++;
  rejects(() => verifyTimeResponse(canonicalize(forged), f.context), 'SIGNATURE_INVALID');
});

test('old Root without time keeps metadata verification but grants no clock authority', () => {
  const f = fixture();
  verifyEnvelope(f.set.bytes.timestamp, { trustedRoot: f.set.metadata.root, now: NOW,
    expected: { role: 'timestamp', product: 'pinvou', component: 'app', scope: {} } });
  f.root.roles = f.root.roles.filter((entry) => entry.role !== 'time'); f.bindRoot();
  rejects(() => verifyTimeResponse(f.bytes(), f.context), 'ROLE_NOT_AUTHORIZED');
  rejects(() => verifyEnvelope(f.bytes(), { trustedRoot: f.root, now: f.claims.serverTime,
    expected: { role: 'time', product: 'pinvou', component: 'app', scope: {} } }), 'TIME_BOOTSTRAP_REQUIRED');
  rejects(() => verifyTimeResponse(f.set.bytes.timestamp, f.context), 'SCHEMA_INVALID');
});

test('time policy dimensions and response scope/purpose cannot impersonate other roles', () => {
  for (const delta of [{ component: null }, { channel: 'stable' }, { targetKey: 'linux-x86_64-deb-perMachine' }]) {
    const f = fixture(); Object.assign(f.root.roles.find((entry) => entry.role === 'time'), delta);
    rejects(() => f.bindRoot(), 'SCHEMA_INVALID');
  }
  const f = fixture();
  for (const delta of [{ scope: { channel: 'stable' } }, { purpose: 'check' }, { component: null }]) {
    const envelope = parseJson(f.bytes()); Object.assign(envelope.signed, delta);
    rejects(() => verifyTimeResponse(canonicalize(envelope), f.context), 'SCHEMA_INVALID');
  }
});

test('the entire UTC interval must fit final Root half-open validity, including equality edges', () => {
  const f = fixture();
  f.root.issuedAt = NOW + 95; f.root.expiresAt = NOW + 126; f.bindRoot();
  verifyTimeResponse(f.bytes(), f.context);
  f.root.expiresAt = NOW + 125; f.bindRoot();
  rejects(() => verifyTimeResponse(f.bytes(), f.context), 'TIME_ROOT_WINDOW_CLOSED');
  f.root.issuedAt = NOW + 96; f.root.expiresAt = NOW + 126; f.bindRoot();
  rejects(() => verifyTimeResponse(f.bytes(), f.context), 'TIME_ROOT_WINDOW_CLOSED');
  f.root.issuedAt = NOW; f.root.expiresAt = NOW + 365 * DAY + 1; f.bindRoot();
  rejects(() => verifyTimeResponse(f.bytes(), f.context), 'TIME_ROOT_WINDOW_INVALID');
});

test('safe-integer interval arithmetic neither rounds overflow nor clips negative UTC', () => {
  const f = fixture();
  f.root.issuedAt = Number.MAX_SAFE_INTEGER - 1000;
  f.root.expiresAt = Number.MAX_SAFE_INTEGER; f.bindRoot();
  f.claims.serverTime = Number.MAX_SAFE_INTEGER - 30;
  const valid = verifyTimeResponse(f.bytes(), f.context);
  assert.equal(valid.latest, Number.MAX_SAFE_INTEGER - 5);
  f.claims.serverTime = Number.MAX_SAFE_INTEGER - 20;
  rejects(() => verifyTimeResponse(f.bytes(), f.context), 'TIME_INTERVAL_OVERFLOW');
  f.claims.serverTime = 0;
  rejects(() => verifyTimeResponse(f.bytes(), f.context), 'TIME_INTERVAL_OVERFLOW');
});

test('canonical response and actual per-object Root/time byte limits are mandatory', () => {
  const f = fixture(), bytes = f.bytes();
  rejects(() => verifyTimeResponse(Buffer.concat([Buffer.from(bytes), Buffer.from('\n')]), f.context),
    'ENVELOPE_NOT_CANONICAL');
  rejects(() => verifyTimeResponse(Buffer.alloc(65_537, 32), f.context), 'JSON_INPUT_LIMIT');
  rejects(() => verifyTimeResponse(bytes, { ...f.context, trustedRootBytes: Buffer.alloc(1_048_577, 32) }),
    'JSON_INPUT_LIMIT');
  const duplicate = Buffer.from(Buffer.from(bytes).toString('utf8').replace('"purpose":', '"serverTime":0,"purpose":'));
  rejects(() => verifyTimeResponse(duplicate, f.context));
});

test('real Root rotation crosses expired old Root only through continuous dual thresholds', () => {
  const vectors = createTimeVectors();
  for (const vector of vectors.cases) {
    assert.deepEqual(verifyTimeResponse(Buffer.from(vector.responseBytes), vectorContext(vector)), vector.expected);
  }
  const vector = vectors.cases[0];
  const context = vectorContext(vector);
  const next = parseJson(context.rootSuccessors[0]);
  const old = parseJson(context.trustedRootBytes);
  const oldIDs = new Set(old.signed.roles.find((entry) => entry.role === 'root').keyIds);
  for (const keep of [(signature) => oldIDs.has(signature.keyId), (signature) => !oldIDs.has(signature.keyId)]) {
    const partial = canonicalize({ ...next, signatures: next.signatures.filter(keep) });
    rejects(() => verifyTimeResponse(Buffer.from(vector.responseBytes), { ...context,
      rootSuccessors: [partial] }), 'SIGNATURE_THRESHOLD_UNMET');
  }
  rejects(() => verifyTimeResponse(Buffer.from(vector.responseBytes), { ...context,
    rootSuccessors: [context.rootSuccessors[0], context.rootSuccessors[0]] }), 'ROOT_CONTINUITY_INVALID');
  rejects(() => verifyTimeResponse(Buffer.from(vector.responseBytes), { ...context,
    deniedKeyIds: [next.signatures[0].keyId] }), 'SIGNING_KEY_DENIED');
  rejects(() => verifyTimeResponse(Buffer.from(vector.responseBytes), { ...context, rootSuccessors: [] }),
    'TIME_ROOT_BINDING_INVALID');
});

test('fixed T26 API uses direct time Envelope and never applies credential iat semantics', () => {
  const f = fixture(), value = parseJson(f.bytes());
  assert.equal(assertApiContract('time', 'request', f.request), f.request);
  assert.equal(assertApiContract('time', 'response', value), value);
  const oversized = { signed: value.signed, signatures: Array(400).fill(value.signatures[0]) };
  rejects(() => assertApiContract('time', 'response', oversized), 'TIME_BODY_TOO_LARGE');
  rejects(() => assertApiContract('time', 'response', { protocolVersion: 1, requestId: 'request-1', response: value }),
    'API_CONTRACT_INVALID');
  rejects(() => assertApiContract('time', 'request', { ...f.request, scope: {} }), 'API_CONTRACT_INVALID');
  const post = openapi.paths['/v1/upgrade/time'].post;
  assert.equal(post.operationId, 'time'); assert.equal(post['x-owner-task'], 'T26');
  assert.equal(post['x-maximum-response-bytes'], 65_536);
  assert.equal(post['x-maximum-request-bytes'], 65_536);
  assert.deepEqual(post.security, []);
});

test('committed public time vectors exercise the same direct and successor contracts', async () => {
  const vectors = JSON.parse(await readFile(new URL('../../vectors/time/anchor.json', import.meta.url), 'utf8'));
  assert.deepEqual(Object.keys(vectors).sort(compare), ['boundary', 'cases', 'protocolVersion']);
  assert.equal(vectors.protocolVersion, 1);
  assert.deepEqual(vectors.cases.map((vector) => vector.name), [
    'expired-root-continuous-successor', 'protected-final-root-with-historical-cross-signatures',
  ]);
  for (const vector of vectors.cases) {
    assert.deepEqual(Object.keys(vector).sort(compare), ['deniedKeyIds', 'expected', 'maxRoundTripMs',
      'maxUncertaintyMs', 'name', 'receivedMonotonicMs', 'request', 'responseBytes', 'rootSuccessors',
      'sentMonotonicMs', 'trustedRootBytes', 'trustedTimeFloor']);
    const response = Buffer.from(vector.responseBytes);
    assert.deepEqual(verifyTimeResponse(response, vectorContext(vector)), vector.expected);
    assertApiContract('time', 'request', vector.request);
    assertApiContract('time', 'response', parseJson(response));
  }
  assert.equal(vectors.cases[0].rootSuccessors[0], vectors.cases[1].trustedRootBytes);
  assert.equal(vectors.cases[0].responseBytes, vectors.cases[1].responseBytes);
});
