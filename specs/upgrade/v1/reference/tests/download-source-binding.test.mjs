import test from 'node:test';
import assert from 'node:assert/strict';
import { sign } from 'node:crypto';
import { canonicalize, parseJson } from '../canonical-json.mjs';
import { assertApiContract } from '../api-registry.mjs';
import { sha256 } from '../digests.mjs';
import { signingInput, verifyEnvelope } from '../signatures.mjs';
import { createFixtureSet, DOWNLOAD_URL, NOW } from './fixtures.mjs';

const set = createFixtureSet();
const claims = set.claims.download;
const expected = { role: claims.role, product: claims.product, component: claims.component, scope: claims.scope };
const verify = (bytes, options = {}) => verifyEnvelope(bytes,
  { trustedRoot: set.metadata.root, expected, now: NOW, ...options });
const response = (credential, url = DOWNLOAD_URL) => ({ protocolVersion: 1, requestId: 'request-1',
  credential, packageId: credential.signed.packageId, size: credential.signed.package.size,
  sha256: credential.signed.package.sha256, url, expiresAt: credential.signed.exp, fileSourceClass: 'primary' });
// Sign deliberately malformed claims independently of the schema-enforcing signer,
// so rejection tests exercise the receiving verifier, not just signing preflight.
const rawSigned = (signed) => canonicalize({ signed, signatures: [{ keyId: set.descriptors[3].keyId,
  signature: sign(null, signingInput(signed), set.keys[3]).toString('base64url') }] });

test('download source binding is mandatory, closed and uses the existing opaque identifier bounds', () => {
  const envelope = verify(set.sign(claims));
  assert.deepEqual(JSON.parse(JSON.stringify(envelope.signed.sourceBinding)), claims.sourceBinding);
  const reject = (mutate) => {
    const invalid = structuredClone(claims); mutate(invalid);
    assert.throws(() => verify(rawSigned(invalid)), { code: 'SCHEMA_INVALID' });
    assert.throws(() => assertApiContract('download-info', 'response', response(parseJson(rawSigned(invalid)))),
      { code: 'API_CONTRACT_INVALID' });
  };
  reject((value) => { delete value.sourceBinding; });
  reject((value) => { value.sourceBinding = null; });
  reject((value) => { value.sourceBinding.extra = 'not-allowed'; });
  for (const key of Object.keys(claims.sourceBinding)) {
    reject((value) => { delete value.sourceBinding[key]; });
    reject((value) => { value.sourceBinding[key] = 1; });
  }
  for (const key of ['sourceId', 'objectId', 'objectVersion']) {
    for (const invalid of ['', 'a'.repeat(129), '/physical/path', 'has space', 'opaque\u0000id', 'source-1\n', 'source-1\r\n', 'source-1\u2028', 'source-1\u2029', 'a'.repeat(128) + '\n']) {
      reject((value) => { value.sourceBinding[key] = invalid; });
    }
    const boundary = structuredClone(claims); boundary.sourceBinding[key] = 'a'.repeat(128);
    verify(set.sign(boundary));
  }
  for (const invalid of ['a'.repeat(63), 'A'.repeat(64), 'g'.repeat(64), 'a'.repeat(64) + '\n', 'a'.repeat(64) + '\r\n', 'a'.repeat(64) + '\u2028', 'a'.repeat(64) + '\u2029']) {
    reject((value) => { value.sourceBinding.urlSha256 = invalid; });
  }
});

test('all four source binding fields are authenticated by the original credential signature', () => {
  const original = parseJson(set.sign(claims));
  for (const key of Object.keys(claims.sourceBinding)) {
    const changed = structuredClone(original);
    changed.signed.sourceBinding[key] = key === 'urlSha256' ? 'f'.repeat(64) : 'other-identity';
    assert.throws(() => verify(canonicalize(changed)), { code: 'SIGNATURE_INVALID' });
  }
});

test('download-info binds exact raw URL UTF-8, including normalization-equivalent strings', () => {
  const credential = verify(set.sign(claims));
  assertApiContract('download-info', 'response', response(credential));
  for (const url of ['https://EXAMPLE.test/package', 'https://example.test:443/package',
    'https://example.test/%70ackage', 'https://example.test/package?', 'https://example.test/other']) {
    assert.throws(() => assertApiContract('download-info', 'response', response(credential, url)),
      { code: 'API_RESPONSE_MISMATCH' });
    const rebound = structuredClone(claims);
    rebound.sourceBinding.urlSha256 = sha256(Buffer.from(url, 'utf8'));
    const signed = verify(set.sign(rebound));
    assertApiContract('download-info', 'response', response(signed, url));
  }
  const unicodeUrl = 'https://example.test/\u00e9';
  const rebound = structuredClone(claims);
  rebound.sourceBinding.urlSha256 = sha256(Buffer.from(unicodeUrl, 'utf8'));
  const signed = verify(set.sign(rebound));
  assertApiContract('download-info', 'response', response(signed, unicodeUrl));
  for (const differentBytes of ['https://example.test/e\u0301', 'https://example.test/%C3%A9']) {
    assert.throws(() => assertApiContract('download-info', 'response', response(signed, differentBytes)),
      { code: 'API_RESPONSE_MISMATCH' });
  }
});

test('source binding preserves package, scope, time, deny and signature checks', () => {
  const bytes = set.sign(claims);
  const credential = verify(bytes);
  for (const change of [{ packageId: 'other-package' }, { size: claims.package.size + 1 },
    { sha256: 'f'.repeat(64) }, { expiresAt: claims.exp + 1 }]) {
    assert.throws(() => assertApiContract('download-info', 'response', { ...response(credential), ...change }),
      { code: 'API_RESPONSE_MISMATCH' });
  }
  const wrongPackage = structuredClone(claims); wrongPackage.packageId = 'other-package';
  assert.throws(() => verify(set.sign(wrongPackage)), { code: 'PACKAGE_BINDING_INVALID' });
  assert.throws(() => verify(bytes, { expected: { ...expected, scope: { ...expected.scope, channelRevision: 2 } } }),
    { code: 'CONTEXT_MISMATCH' });
  assert.throws(() => verify(bytes, { now: claims.exp }), { code: 'TIME_WINDOW_CLOSED' });
  assert.throws(() => verify(bytes, { deniedKeyIds: [set.descriptors[3].keyId] }), { code: 'SIGNING_KEY_DENIED' });
  const changed = parseJson(bytes); changed.signatures[0].signature = 'A'.repeat(86);
  assert.throws(() => verify(canonicalize(changed)), { code: 'SIGNATURE_INVALID' });
});
