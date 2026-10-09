import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { canonicalize, parseJson } from '../canonical-json.mjs';
import { domainBytes, sha256, consumeRequestDigest, statusBindingHash,
  sourceProfileId, rolloutCommitment, recoveryHandleHash } from '../digests.mjs';
import { signingInput, verifyEnvelope } from '../signatures.mjs';

const envelopes = JSON.parse(await readFile(new URL('../../vectors/crypto/envelopes.json', import.meta.url), 'utf8'));
const digests = JSON.parse(await readFile(new URL('../../vectors/crypto/digests.json', import.meta.url), 'utf8'));

test('twenty independently verified fixed envelopes match signature inputs, hashes and claims', () => {
  assert.equal(envelopes.vectors.length, 20);
  for (const vector of envelopes.vectors) {
    const bytes = Buffer.from(vector.envelopeBase64Url, 'base64url');
    const envelope = verifyEnvelope(bytes, { trustedRoot: envelopes.trustedRoot,
      expected: vector.expectedContext, now: vector.now });
    assert.equal(sha256(bytes), vector.expectedEnvelopeSha256);
    assert.equal(signingInput(envelope.signed).toString('hex'), vector.expectedSigningInputHex);
  }
});

test('every fixed signed scalar rejects changed content, including nested scope and chain identities', () => {
  function leaves(value, path = []) {
    if (value === null || typeof value !== 'object') return [path];
    return Object.entries(value).flatMap(([key, child]) => leaves(child, [...path, key]));
  }
  for (const vector of envelopes.vectors) {
    const original = parseJson(Buffer.from(vector.envelopeBase64Url, 'base64url'));
    for (const path of leaves(original.signed)) {
      const changed = structuredClone(original);
      let parent = changed.signed;
      for (const key of path.slice(0, -1)) parent = parent[key];
      const key = path.at(-1);
      const value = parent[key];
      parent[key] = typeof value === 'number' ? value + 1
        : typeof value === 'boolean' ? !value : value === null ? 'tampered' : value + '-tampered';
      assert.throws(() => verifyEnvelope(canonicalize(changed), { trustedRoot: envelopes.trustedRoot,
        expected: vector.expectedContext, now: vector.now }), (error) => typeof error.code === 'string',
      `${vector.name}: ${path.join('.')}`);
    }
  }
});

test('independent digest known answers freeze exact NUL, projections and semantic bindings', () => {
  assert.equal(sourceProfileId(digests.sourceFacts), digests.expectedSourceProfileId);
  assert.equal(consumeRequestDigest(digests.consumeRequest), digests.expectedConsumeRequestDigest);
  assert.equal(statusBindingHash(digests.statusBinding), digests.expectedStatusBindingHash);
  assert.equal(domainBytes('pinvou-consume-status-v1', digests.statusBinding).toString('hex'), digests.expectedStatusInputHex);
  assert.notEqual(digests.expectedStatusBindingHash, digests.expectedEscapedNulHash);
  assert.equal(rolloutCommitment(null), digests.expectedEmptyCommitment);
  assert.equal(rolloutCommitment(digests.opening), digests.expectedOpeningCommitment);
  assert.equal(recoveryHandleHash(Buffer.alloc(32, 1)), digests.consumeRequest.recoveryHandleHash);
  assert.throws(() => recoveryHandleHash(Buffer.alloc(31)), { code: 'SECRET_SIZE_INVALID' });
});

test('source profile excludes policy/runtime fields but includes every stable native fact', () => {
  const facts = { ...digests.sourceFacts, sourceProfileId: 'ignored', registryVersion: 99,
    ownerEpoch: 1, freezeEpoch: 4, transactionId: 'dynamic', permissionState: 'changed', policyId: 'changed' };
  assert.equal(sourceProfileId(facts), digests.expectedSourceProfileId);
  const changed = structuredClone(facts); changed.ownerEpoch++;
  assert.equal(sourceProfileId(changed), sourceProfileId(facts));
  assert.notEqual(sha256(canonicalize(changed)), sha256(canonicalize(facts)));
  for (const field of ['application', 'helper', 'launcher']) {
    const changed = structuredClone(facts); changed[field].sha256 = 'f'.repeat(64);
    assert.notEqual(sourceProfileId(changed), digests.expectedSourceProfileId);
  }
  const nestedEpoch = structuredClone(facts); nestedEpoch.helper.ownerEpoch = 1;
  assert.throws(() => sourceProfileId(nestedEpoch), { code: 'SCHEMA_INVALID' });
});

test('consume digest excludes only named transport/self fields; all actual semantic fields remain bound', () => {
  const request = { ...digests.consumeRequest, consumeRequestDigest: 'self', recoverySecret: 'not-transmitted',
    requestId: 'transport-only', headers: { Authorization: 'never-sent-to-file-service' } };
  assert.equal(consumeRequestDigest(request), digests.expectedConsumeRequestDigest);
  const changed = structuredClone(request); changed.preparation.ownerEpoch++;
  assert.notEqual(consumeRequestDigest(changed), digests.expectedConsumeRequestDigest);
  for (const field of Object.keys(digests.statusBinding)) {
    const missing = { ...digests.statusBinding }; delete missing[field];
    assert.throws(() => statusBindingHash(missing), { code: 'SCHEMA_INVALID' });
    const changed = { ...digests.statusBinding, [field]: field.endsWith('Hash') || field.endsWith('Digest')
      ? 'f'.repeat(64) : field === 'purpose' ? 'activate' : digests.statusBinding[field] + '-other' };
    assert.notEqual(statusBindingHash(changed), digests.expectedStatusBindingHash);
  }
});
