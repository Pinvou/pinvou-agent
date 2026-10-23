import test from 'node:test';
import assert from 'node:assert/strict';
import { assertApiContract, assertApiShape } from '../api-registry.mjs';

// Structural contract controls only. These objects confer no native authority,
// actual key absence, cryptographic source validity or original COMMIT result.
const hash = 'a'.repeat(64);
const ref = revision => ({ kind: 'rollout', id: 'child-1', revision, sha256: hash });
const proof = () => ({ protocolVersion: 1, status: 'not_committable', proof: {
  operation: 'abort', command: 'rollout.abort.v1', actorId: 'actor-1',
  scope: { product: 'pinvou', component: 'desktop', channel: 'stable', targetKey: 'linux-x86_64-deb-perMachine' },
  commandKey: 'original-abort', publicRequestSha256: hash, internalWireSha256: 'b'.repeat(64),
  basis: { kind: 'rollout_closed', original: ref(1), current: ref(2), closingResultId: 'c'.repeat(64) } } });
const invalid = value => assert.throws(() => assertApiContract('deployment-rollout-lookup', 'response', value), { code: 'API_CONTRACT_INVALID' });

test('absent-original abort proof has a distinct closed locator variant', () => {
  const value = proof();
  assert.equal(assertApiContract('deployment-rollout-lookup', 'response', value), value);
  assert.equal(assertApiShape('deployment-rollout.abort-not-committable', value), value);
  invalid({ ...value, resultId: hash });
});
test('abort closed-child proof cannot substitute create or approval predicates', () => {
  for (const [field, value] of [['operation', 'create'], ['command', 'deployment.create.v1'], ['command', 'prepare-abort']]) {
    const result = proof(); result.proof[field] = value; invalid(result);
  }
  for (const kind of ['parent_advanced', 'authorization_advanced', 'unresolved']) {
    const result = proof(); result.proof.basis.kind = kind; invalid(result);
  }
});
test('abort proof requires both actual child revisions and a closing result identity', () => {
  for (const [field, revision] of [['original', 2], ['current', 1], ['current', 3]]) {
    const result = proof(); result.proof.basis[field].revision = revision; invalid(result);
  }
  for (const value of [null, '', 'F'.repeat(64), 'a'.repeat(63)]) {
    const result = proof(); result.proof.basis.closingResultId = value; invalid(result);
  }
  const wrongKind = proof(); wrongKind.proof.basis.current.kind = 'deployment'; invalid(wrongKind);
});
test('abort proof wire cannot lose original binding fields or add unsigned claims', () => {
  for (const field of Object.keys(proof().proof)) {
    const missing = proof(); delete missing.proof[field]; invalid(missing);
    const nullField = proof(); nullField.proof[field] = null; invalid(nullField);
  }
  for (const [field, value] of [['permission', 'change.create'], ['committed', true], ['response', {}]]) {
    const extra = proof(); extra.proof[field] = value; invalid(extra);
  }
});
